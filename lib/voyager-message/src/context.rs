use std::{collections::HashMap, process::Stdio, sync::Arc, time::Duration};

use futures::{stream::FuturesUnordered, Future, StreamExt, TryStreamExt};
use jsonrpsee::types::{ErrorObject, ErrorObjectOwned};
use macros::model;
use serde_json::Value;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, debug_span, error, info, instrument, trace, warn, Instrument};
use unionlabs::{ethereum::keccak256, hash::hash_v2::HexUnprefixed, traits::Member, ErrorReporter};
use voyager_core::ConsensusType;
use voyager_vm::{BoxDynError, QueueError};

use crate::{
    core::{ChainId, ClientType, IbcInterface},
    module::{
        ChainModuleClient, ChainModuleInfo, ClientModuleClient, ClientModuleInfo,
        ConsensusModuleClient, ConsensusModuleInfo, ModuleInfo, ModuleKindInfo, PluginClient,
        PluginInfo,
    },
    rpc::{server::Server, VoyagerRpcServer},
    FATAL_JSONRPC_ERROR_CODE,
};

pub const INVALID_CONFIG_EXIT_CODE: u8 = 13;
pub const STARTUP_ERROR_EXIT_CODE: u8 = 14;

#[derive(Debug)]
pub struct Context {
    pub rpc_server: Server,

    plugins: HashMap<String, ModuleRpcClient>,

    interest_filters: HashMap<String, String>,

    cancellation_token: CancellationToken,
    // module_servers: Vec<ModuleRpcServer>,
}

#[derive(Debug, Clone)]
pub struct Modules {
    /// map of chain id to chain module.
    chain_modules: HashMap<ChainId<'static>, ModuleRpcClient>,

    /// map of chain id to consensus module.
    consensus_modules: HashMap<ChainId<'static>, ModuleRpcClient>,

    /// map of client type to ibc interface to client module.
    client_modules: HashMap<ClientType<'static>, HashMap<IbcInterface<'static>, ModuleRpcClient>>,

    chain_consensus_types: HashMap<ChainId<'static>, ConsensusType<'static>>,

    client_consensus_types: HashMap<ClientType<'static>, ConsensusType<'static>>,
}

impl voyager_vm::Context for Context {
    fn tags(&self) -> Vec<&str> {
        self.interest_filters.keys().map(|s| s.as_str()).collect()
    }
}

#[derive(macros::Debug, Clone)]
pub struct ModuleRpcClient {
    #[debug(skip)]
    client: reconnecting_jsonrpc_ws_client::Client,
    #[allow(dead_code)]
    name: String,
}

impl ModuleRpcClient {
    fn new(name: &str) -> Self {
        let socket = Self::make_socket_path(name);

        let client = reconnecting_jsonrpc_ws_client::Client::new({
            // NOTE: This needs to be leaked because the return type of the .build() method
            // below captures the lifetime of the `name` parameter(?)
            let socket: &'static str = Box::leak(socket.clone().into_boxed_str());
            let name = name.to_owned();
            move || {
                async move {
                    trace!("connecting to socket at {socket}");
                    reth_ipc::client::IpcClientBuilder::default()
                        .build(socket)
                        .await
                }
                .instrument(debug_span!("module_ipc_client", %name))
            }
        });

        Self {
            client,
            name: name.to_owned(),
        }
    }

    fn make_socket_path(name: &str) -> String {
        format!(
            "/tmp/voyager-to-module-{}.sock",
            keccak256(name).into_encoding::<HexUnprefixed>()
        )
    }

    pub fn client(&self) -> &impl jsonrpsee::core::client::ClientT {
        &self.client
    }
}

async fn module_rpc_server(
    name: &str,
    server: Server,
) -> Result<impl Future<Output = ()>, BoxDynError> {
    let socket = make_module_rpc_server_socket_path(name);
    let rpc_server = reth_ipc::server::Builder::default().build(socket.clone());

    info!(%socket, "starting rpc server");

    let server = rpc_server.start(server.into_rpc()).await?;

    Ok(server
        .stopped()
        .instrument(debug_span!("module_rpc_server", %name)))
}

fn make_module_rpc_server_socket_path(name: &str) -> String {
    format!(
        "/tmp/module-to-voyager-{}.sock",
        keccak256(name).into_encoding::<HexUnprefixed>()
    )
}

#[model]
#[derive(Hash)]
pub struct ModuleConfig {
    pub path: String,
    pub config: Value,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

impl Context {
    #[instrument(name = "context_new", skip_all)]
    pub async fn new(module_configs: Vec<ModuleConfig>) -> Result<Self, BoxDynError> {
        let cancellation_token = CancellationToken::new();

        let mut modules = Modules {
            chain_modules: Default::default(),
            client_modules: Default::default(),
            consensus_modules: Default::default(),
            chain_consensus_types: Default::default(),
            client_consensus_types: Default::default(),
        };

        let mut plugins = HashMap::default();

        let mut interest_filters = HashMap::default();

        let main_rpc_server = Server::new();

        let module_configs = module_configs
            .into_iter()
            .filter(|module_config| {
                if !module_config.enabled {
                    info!(module_path = %module_config.path, "module is not enabled, skipping");
                    false
                } else {
                    true
                }
            })
            .collect::<Vec<_>>();

        info!("fetching module info");

        let module_configs = module_configs
            .into_iter()
            .map(|module_config| {
                let server = main_rpc_server.clone();
                async move {
                    let module_info = get_module_info(&module_config)?;

                    info!("starting rpc server for {}", module_info.kind.name());
                    tokio::spawn(module_rpc_server(&module_info.kind.name(), server).await?);

                    Ok::<_, BoxDynError>((module_config, module_info))
                }
            })
            .collect::<FuturesUnordered<_>>()
            .try_collect::<Vec<_>>()
            .await?;

        for (module_config, ModuleInfo { kind }) in module_configs {
            let name = kind.name();

            info!("registering module {}", name);

            // tokio::spawn(module_rpc_server(&name, main_rpc_server.clone()).await?);

            tokio::spawn(run_module(
                name.clone(),
                module_config.clone(),
                cancellation_token.clone(),
            ));

            let rpc_client = ModuleRpcClient::new(&name);

            plugins.insert(name.clone(), rpc_client.clone());

            match kind {
                ModuleKindInfo::Plugin(PluginInfo {
                    name,
                    interest_filter,
                }) => {
                    plugins.insert(name.clone(), rpc_client.clone());

                    info!(
                        %name,
                        "registered plugin module"
                    );

                    interest_filters.insert(name.clone(), interest_filter);
                }
                ModuleKindInfo::Chain(ChainModuleInfo { chain_id }) => {
                    let prev = modules.chain_modules.insert(chain_id.clone(), rpc_client);

                    if prev.is_some() {
                        return Err(format!(
                            "multiple chain modules configured for chain id `{chain_id}`"
                        )
                        .into());
                    }

                    info!(
                        %chain_id,
                        "registered chain module"
                    );
                }
                ModuleKindInfo::Client(ClientModuleInfo {
                    client_type,
                    consensus_type,
                    ibc_interface,
                }) => {
                    let prev = modules
                        .client_modules
                        .entry(client_type.clone())
                        .or_default()
                        .insert(ibc_interface.clone(), rpc_client.clone());

                    if prev.is_some() {
                        return Err(format!(
                            "multiple client modules configured for \
                            client type `{client_type}` and IBC \
                            interface `{ibc_interface}`",
                        )
                        .into());
                    }

                    if let Some(previous_consensus_type) = modules
                        .client_consensus_types
                        .insert(client_type.clone(), consensus_type.clone())
                    {
                        if previous_consensus_type != consensus_type {
                            return Err(format!(
                                "inconsistency in client consensus types: \
                                client type `{client_type}` is registered \
                                as tracking both `{previous_consensus_type}` \
                                and `{consensus_type}`"
                            )
                            .into());
                        }
                    }

                    info!(
                        %client_type,
                        %ibc_interface,
                        "registered client module"
                    );
                }
                ModuleKindInfo::Consensus(ConsensusModuleInfo {
                    chain_id,
                    consensus_type,
                }) => {
                    let prev = modules
                        .consensus_modules
                        .insert(chain_id.clone(), rpc_client.clone());

                    if prev.is_some() {
                        return Err(format!(
                            "multiple consensus modules configured for chain id `{chain_id}`"
                        )
                        .into());
                    }

                    let None = modules
                        .chain_consensus_types
                        .insert(chain_id.clone(), consensus_type.clone())
                    else {
                        unreachable!()
                    };

                    info!(
                        %chain_id,
                        %consensus_type,
                        "registered consensus module"
                    );
                }
            }
        }

        main_rpc_server.start(Arc::new(modules));

        info!("checking for plugin health...");

        {
            let mut futures = plugins
                .iter()
                .map(|(name, client)| async move {
                    match client
                        .client
                        .wait_until_connected(Duration::from_secs(10))
                        .instrument(debug_span!("health check", %name))
                        .await
                    {
                        Ok(_) => {
                            info!("plugin {name} connected")
                        }
                        Err(_) => {
                            warn!("plugin {name} failed to connect after 10 seconds")
                        }
                    }
                })
                .collect::<FuturesUnordered<_>>();

            while let Some(()) = futures.next().await {}
        }

        Ok(Self {
            rpc_server: main_rpc_server,
            plugins,
            interest_filters,
            cancellation_token,
            // module_servers,
        })
    }

    pub async fn shutdown(self) {
        self.cancellation_token.cancel();

        for (name, client) in self.plugins {
            debug!("shutting down plugin client for {name}");
            client.client.shutdown();
        }
    }

    pub fn plugin<D: Member, C: Member, Cb: Member>(
        &self,
        name: impl AsRef<str>,
    ) -> Result<&(impl PluginClient<D, C, Cb> + '_), PluginNotFound> {
        Ok(self
            .plugins
            .get(name.as_ref())
            .ok_or_else(|| PluginNotFound {
                name: name.as_ref().into(),
            })?
            .client())
    }

    pub fn plugin_client_raw(
        &self,
        name: impl AsRef<str>,
    ) -> Result<&ModuleRpcClient, PluginNotFound> {
        self.plugins
            .get(name.as_ref())
            .ok_or_else(|| PluginNotFound {
                name: name.as_ref().into(),
            })
    }

    pub fn interest_filters(&self) -> &HashMap<String, String> {
        &self.interest_filters
    }
}

impl Modules {
    pub fn info(&self) -> LoadedModulesInfo {
        let chain = self
            .chain_modules
            .keys()
            .cloned()
            .map(|chain_id| ChainModuleInfo { chain_id })
            .collect();

        let consensus = self
            .consensus_modules
            .keys()
            .cloned()
            .map(|chain_id| ConsensusModuleInfo {
                consensus_type: self.chain_consensus_types[&chain_id].clone(),
                chain_id,
            })
            .collect();

        let client = self
            .client_modules
            .iter()
            .map(|(client_type, ibc_interfaces)| (client_type, ibc_interfaces.keys()))
            .flat_map(|(client_type, ibc_interfaces)| {
                ibc_interfaces
                    .cloned()
                    .map(move |ibc_interface| ClientModuleInfo {
                        consensus_type: self.client_consensus_types[client_type].clone(),
                        client_type: client_type.clone(),
                        ibc_interface,
                    })
            })
            .collect();

        LoadedModulesInfo {
            chain,
            consensus,
            client,
        }
    }

    pub fn chain_consensus_type<'a, 'b, 'c: 'a>(
        &'a self,
        chain_id: &'b ChainId<'c>,
    ) -> Result<&'a ConsensusType<'static>, ConsensusModuleNotFound> {
        self.chain_consensus_types
            .get(chain_id)
            .ok_or_else(|| ConsensusModuleNotFound(chain_id.clone().into_owned()))
    }

    pub fn client_consensus_type<'a, 'b, 'c: 'a>(
        &'a self,
        client_type: &'b ClientType<'c>,
    ) -> Result<&'a ConsensusType<'static>, ClientModuleNotFound> {
        self.client_consensus_types.get(client_type).ok_or_else(|| {
            ClientModuleNotFound::ClientTypeNotFound {
                client_type: client_type.clone().into_owned(),
            }
        })
    }

    pub fn chain_module<'a, 'b, 'c: 'a>(
        &'a self,
        chain_id: &'b ChainId<'c>,
    ) -> Result<&'a (impl ChainModuleClient + 'a), ChainModuleNotFound> {
        Ok(self
            .chain_modules
            .get(chain_id)
            .ok_or_else(|| ChainModuleNotFound(chain_id.clone().into_owned()))?
            .client())
    }

    pub fn consensus_module<'a, 'b, 'c: 'a>(
        &'a self,
        chain_id: &'b ChainId<'c>,
    ) -> Result<&'a (impl ConsensusModuleClient + 'a), ConsensusModuleNotFound> {
        Ok(self
            .consensus_modules
            .get(chain_id)
            .ok_or_else(|| ConsensusModuleNotFound(chain_id.clone().into_owned()))?
            .client())
    }

    pub fn client_module<'a, 'b, 'c: 'a>(
        &'a self,
        client_type: &'b ClientType<'c>,
        ibc_interface: &'b IbcInterface<'c>,
    ) -> Result<&'a (impl ClientModuleClient + 'a), ClientModuleNotFound> {
        match self.client_modules.get(client_type) {
            Some(ibc_interfaces) => match ibc_interfaces.get(ibc_interface) {
                Some(client_module) => Ok(client_module.client()),
                None => Err(ClientModuleNotFound::IbcInterfaceNotFound {
                    client_type: client_type.clone().into_owned(),
                    ibc_interface: ibc_interface.clone().into_owned(),
                }),
            },
            None => Err(ClientModuleNotFound::ClientTypeNotFound {
                client_type: client_type.clone().into_owned(),
            }),
        }
    }
}

#[model]
pub struct LoadedModulesInfo {
    chain: Vec<ChainModuleInfo>,
    consensus: Vec<ConsensusModuleInfo>,
    client: Vec<ClientModuleInfo>,
}

#[instrument(skip_all, fields(%name))]
async fn run_module(
    name: String,
    module_config: ModuleConfig,
    cancellation_token: CancellationToken,
) {
    let client_socket = ModuleRpcClient::make_socket_path(&name);
    let server_socket = make_module_rpc_server_socket_path(&name);

    info!(%client_socket, %server_socket, "starting module {name}");

    let mut attempt = 0;

    loop {
        debug!(%attempt, "spawning plugin child process");

        let mut cmd = tokio::process::Command::new(&module_config.path);
        cmd.arg("run");
        cmd.arg(&client_socket);
        cmd.arg(&server_socket);
        cmd.arg(module_config.config.to_string());

        let mut child = loop {
            match cmd.spawn() {
                Ok(child) => {
                    let id = child.id().unwrap();

                    info!(%id, "spawned plugin");

                    break child;
                }
                Err(err) => {
                    error!(
                        err = %ErrorReporter(err),
                        path = ?module_config.path,
                        "unable to spawn plugin"
                    );

                    sleep(Duration::from_secs(1)).await;
                }
            }
        };

        let id = child.id().unwrap();

        tokio::select! {
            _ = cancellation_token.cancelled() => {
                debug!(%id, "killing plugin");
                match child.kill().await {
                    Ok(()) => {
                        debug!(%id, "plugin received kill signal");
                        match child.wait().await {
                            Ok(exit_status) => {
                                debug!(%id, %exit_status, "child exited successfully")
                            }
                            Err(err) => {
                                error!(%id, err = %ErrorReporter(err), "child exited unsuccessfully")
                            }
                        }
                    }
                    Err(err) => {
                        error!(%id, err = %ErrorReporter(err), "unable to kill plugin")
                    }
                }

                break
            }
            res = child.wait() => {
                match res {
                    Ok(exit_status) => {
                        info!(%id, %exit_status, "child exited");

                        if exit_status
                            .code()
                            .is_some_and(|c| c == INVALID_CONFIG_EXIT_CODE as i32)
                        {
                            error!(%id, %exit_status, "invalid config for plugin");
                            cancellation_token.cancel();
                        }
                    }
                    Err(err) => {
                        error!(%id, err = %ErrorReporter(err), "child exited");
                    }
                }

                // TODO: Exponential backoff
                sleep(Duration::from_secs(1)).await;
            }
        }

        attempt += 1;
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("no module loaded for chain `{0}`")]
pub struct ChainModuleNotFound(pub ChainId<'static>);

impl From<ChainModuleNotFound> for QueueError {
    fn from(value: ChainModuleNotFound) -> Self {
        Self::Fatal(Box::new(value))
    }
}

impl From<ChainModuleNotFound> for ErrorObjectOwned {
    fn from(value: ChainModuleNotFound) -> Self {
        ErrorObject::owned(FATAL_JSONRPC_ERROR_CODE, value.to_string(), None::<()>)
    }
}

impl From<ChainModuleNotFound> for jsonrpsee::core::client::Error {
    fn from(value: ChainModuleNotFound) -> Self {
        ErrorObject::from(value).into()
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("no module loaded for consensus on chain `{0}`")]
pub struct ConsensusModuleNotFound(pub ChainId<'static>);

impl From<ConsensusModuleNotFound> for QueueError {
    fn from(value: ConsensusModuleNotFound) -> Self {
        Self::Fatal(Box::new(value))
    }
}

impl From<ConsensusModuleNotFound> for jsonrpsee::core::client::Error {
    fn from(value: ConsensusModuleNotFound) -> Self {
        ErrorObject::from(value).into()
    }
}

impl From<ConsensusModuleNotFound> for ErrorObjectOwned {
    fn from(value: ConsensusModuleNotFound) -> Self {
        ErrorObject::owned(FATAL_JSONRPC_ERROR_CODE, value.to_string(), None::<()>)
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ClientModuleNotFound {
    #[error("no module loaded for client type `{}`", client_type)]
    ClientTypeNotFound { client_type: ClientType<'static> },
    #[error(
        "no module loaded supporting IBC interface `{}` and client type `{}`",
        client_type,
        ibc_interface
    )]
    IbcInterfaceNotFound {
        client_type: ClientType<'static>,
        ibc_interface: IbcInterface<'static>,
    },
}

impl From<ClientModuleNotFound> for QueueError {
    fn from(value: ClientModuleNotFound) -> Self {
        Self::Fatal(Box::new(value))
    }
}

impl From<ClientModuleNotFound> for jsonrpsee::core::client::Error {
    fn from(value: ClientModuleNotFound) -> Self {
        ErrorObject::from(value).into()
    }
}

impl From<ClientModuleNotFound> for ErrorObjectOwned {
    fn from(value: ClientModuleNotFound) -> Self {
        ErrorObject::owned(FATAL_JSONRPC_ERROR_CODE, value.to_string(), None::<()>)
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("plugin `{name}` not found")]
pub struct PluginNotFound {
    pub name: String,
}

impl From<PluginNotFound> for QueueError {
    fn from(value: PluginNotFound) -> Self {
        Self::Fatal(Box::new(value))
    }
}

impl From<PluginNotFound> for ErrorObjectOwned {
    fn from(value: PluginNotFound) -> Self {
        ErrorObject::owned(FATAL_JSONRPC_ERROR_CODE, value.to_string(), None::<()>)
    }
}

impl From<PluginNotFound> for jsonrpsee::core::client::Error {
    fn from(value: PluginNotFound) -> Self {
        ErrorObject::from(value).into()
    }
}

pub fn get_module_info(module_config: &ModuleConfig) -> Result<ModuleInfo<ModuleKindInfo>, String> {
    debug!(
        "querying module info from module at {}",
        &module_config.path
    );

    let mut cmd = std::process::Command::new(&module_config.path);
    cmd.arg("info");
    cmd.arg(module_config.config.to_string());

    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
        .wait_with_output()
        .unwrap();

    if !output.status.success() {
        match output.status.code() {
            Some(code) if code == INVALID_CONFIG_EXIT_CODE as i32 => {
                return Err(format!(
                    "invalid config for module at path {}:\n{}",
                    &module_config.path,
                    String::from_utf8_lossy(&output.stdout)
                ));
            }
            Some(_) | None => {
                return Err(format!(
                    "unable to query info for module at path {}:\n{}",
                    &module_config.path,
                    String::from_utf8_lossy(&output.stdout)
                ))
            }
        }
    }

    trace!("plugin stdout: {}", String::from_utf8_lossy(&output.stdout));

    Ok(serde_json::from_slice(&output.stdout).unwrap())
}
