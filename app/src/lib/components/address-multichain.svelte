<script lang="ts">
import type { Chain } from "$lib/types"
import { hexAddressToBech32 } from "@unionlabs/client"
import { Badge } from "$lib/components/ui/badge/index.ts"

export let address: { address: string; normalizedAddress: string }
export let chains: Array<Chain>

const addressChain = chains.find(c => address.address.startsWith(c.addr_prefix)) as Chain

const otherCosmosAddresses: Array<{ address: string; chain: Chain }> = chains
  .filter(chain => chain.rpc_type === "cosmos")
  .map(chain => ({
    address: hexAddressToBech32({
      bech32Prefix: chain.addr_prefix,
      address: `0x${address.normalizedAddress}`
    }),
    chain: chain
  }))
  .filter(pair => pair.address !== address.address)

const allCosmosAddresses = [
  { address: address.address, chain: addressChain },
  ...otherCosmosAddresses
].map(pair => {
  let prefix = pair.chain.addr_prefix
  let body = pair.address.replace(prefix, "").slice(0, -6)
  let checksum = pair.address.slice(-6)

  return {
    prefix,
    body,
    checksum,
    ...pair
  }
})

const allCosmosAddressesDeduplicated = allCosmosAddresses.filter(
  (obj1, i, arr) => arr.findIndex(obj2 => obj2.prefix === obj1.prefix) === i
)

const longestPrefix = Math.max.apply(
  0,
  allCosmosAddresses.map(pair => pair.prefix.length)
)
let addressIndex = 0
setInterval(() => {
  //logic goes here
  addressIndex = (addressIndex + 1) % (allCosmosAddresses.length - 1)
}, 2000)
</script>

{#if addressChain?.rpc_type === "evm"}
  <div class="flex items-center gap-2">
    <div class="text-sm sm:text-base md:text-lg font-bold flex items-center">
      <span class="text-muted-foreground">0x</span>{address.address.slice(2)}
    </div>
    <Badge class="hidden md:block">EVM</Badge>
  </div>
{:else}
  <div class="flex items-center">
    <ul>
      {#each allCosmosAddressesDeduplicated as cosmosAddress, i}
        {#if i === addressIndex}
          <li
            class="text-sm sm:text-base md:text-lg first:font-bold whitespace-pre"
          >
            <span class="select-none"
              >{" ".repeat(longestPrefix - cosmosAddress.prefix.length)}</span
            ><span class="text-muted-foreground mr-1"
              >{cosmosAddress.prefix}</span
            >{cosmosAddress.body}<span class="ml-1 text-muted-foreground"
              >{cosmosAddress.checksum}</span
            >
          </li>
        {/if}
      {/each}
    </ul>
    <Badge class="hidden md:block">Cosmos</Badge>
  </div>
{/if}
