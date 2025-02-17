<script lang="ts">
import { page } from "$app/stores"
import { onNavigate } from "$app/navigation"
import { cn } from "$lib/utilities/shadcn.ts"
import type { LayoutData } from "./$types.ts"
import Menu from "./(components)/menu.svelte"
import StatsBar from "./(components)/stats-bar.svelte"
import ExplorerBreadcrumbs from "$lib/components/explorer-breadcrumbs.svelte"

export let data: LayoutData

let explorerRoute = $page.route.id?.split("/").at(2) ?? null
$: explorerPageDescription =
  data.tables.filter(t => t.route === explorerRoute).at(0)?.description ?? null

onNavigate(navigation => {
  if (navigation.to?.route.id?.split("/").at(1) === "explorer") {
    explorerRoute = navigation.to?.route.id?.split("/").at(2) ?? null
  }
})
</script>

<svelte:head>
  <title>Union - Explorer</title>
</svelte:head>

<div class="flex flex-row sm:divide-x overflow-x-none max-w-full w-full">
  <nav
    class={cn(
      "sm:bg-muted h-full overflow-y-auto",
      explorerRoute === null
        ? "flex-1 sm:flex-none"
        : "hidden sm:block sm:w-[174px]"
    )}
  >
    <h2 class="sm:hidden ml-3 mt-6 mb-3 text-2xl font-bold font-supermolot">
      Explorer
    </h2>
    <Menu tableRoutes={data.tables} />
  </nav>
  <main
    class={cn(
      "overflow-auto flex flex-col flex-1 w-full",
      explorerRoute === null ? "hidden sm:block" : ""
    )}
  >
    {#if $page.route.id?.split("/").length === 3}
      <StatsBar />
    {/if}

    {#if ($page?.route?.id?.split("/")?.length ?? 0) > 3}
        <ExplorerBreadcrumbs/>
    {/if}

    <div class="flex flex-col flex-1 size-full">
      <div class="p-2 sm:p-4 md:p-6">
        <div
          class={cn($page.route.id?.split("/").length === 3 ? "" : "hidden")}
        >
          <h2
            class="text-2xl sm:text-4xl font-extrabold font-expanded sm:!font-extra-expanded uppercase font-supermolot pt-4 sm:pt-0"
          >
            {explorerRoute?.replaceAll("-", " ")}
          </h2>
          <p class="pb-4 -mt-1 text-muted-foreground">
            {">"}
            {explorerPageDescription}
          </p>
        </div>
        <slot />
      </div>
    </div>
  </main>
</div>
