<script lang="ts">
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
	import '../app.css';
	import { onMount } from 'svelte';
	import { Sun, Moon } from '@lucide/svelte';
	import { Button } from '#lib/components/ui/button/index.ts';
	import * as Tooltip from '#lib/components/ui/tooltip/index.ts';
	import { theme } from '#lib/stores/theme.svelte.ts';
	import Logo from '#lib/components/Logo.svelte';
	import AppNav from '#lib/components/shell/AppNav.svelte';
	import CommandMenu from '#lib/components/shell/CommandMenu.svelte';
	import DatasetSwitcher from '#lib/components/shell/DatasetSwitcher.svelte';
	import { pageDatasetId, pageDatasets, pageSection } from '#lib/components/shell/app-context.ts';
	import {
		getCachedDatasetSummaries,
		loadDatasetSummaries,
		type DatasetSummary
	} from '#lib/datasets.ts';

	let { children } = $props();
	let fetchedDatasets = $state.raw<DatasetSummary[] | null>(null);
	const datasets = $derived(pageDatasets(page) ?? fetchedDatasets ?? getCachedDatasetSummaries());
	const datasetId = $derived(pageDatasetId(page));
	const section = $derived(pageSection(page.url.pathname));

	function requestDatasets() {
		void loadDatasetSummaries().then(
			(loaded) => (fetchedDatasets = loaded),
			() => {}
		);
	}

	onMount(() => {
		theme.syncFromDom();
	});
</script>

<!-- Single app-wide provider required by every Tooltip.Root (bits-ui) -->
<Tooltip.Provider>
	<div class="bg-background text-foreground flex h-dvh flex-col overflow-hidden font-sans">
		<header class="app-topbar border-border bg-card shrink-0 border-b">
			<div class="shell flex h-12 items-center gap-1.5">
				<a
					href={resolve('/')}
					class="text-foreground -ml-1 rounded-md px-1 py-1 text-[0.9375rem] font-semibold tracking-tight"
					><Logo /></a
				>
				<span class="text-border text-xl font-light select-none" aria-hidden="true">/</span>
				<DatasetSwitcher {datasets} {datasetId} {section} onRequestDatasets={requestDatasets} />
				<div class="ml-auto flex items-center gap-1">
					<CommandMenu {datasets} {datasetId} onRequestDatasets={requestDatasets} />
					<Button
						variant="ghost"
						size="icon-sm"
						onclick={() => theme.toggle()}
						class="text-muted-foreground hover:text-foreground"
						aria-label={theme.dark ? 'Switch to light mode' : 'Switch to dark mode'}
					>
						{#if theme.dark}
							<Sun class="size-4" />
						{:else}
							<Moon class="size-4" />
						{/if}
					</Button>
				</div>
			</div>
		</header>
		<AppNav {datasetId} active={section} />

		<main class="app-shell__main min-h-0 flex-1 overflow-y-auto">
			{@render children()}
			<footer
				class="text-muted-foreground border-border flex flex-col items-center justify-center gap-1 border-t py-6 text-xs"
			>
				<div class="flex flex-wrap items-center justify-center gap-x-2">
					<a
						href="https://onrg.gitlab.io"
						class="hover:text-foreground"
						target="_blank"
						rel="noopener noreferrer"
					>
						ONRG
					</a>
					<span aria-hidden="true">&middot;</span>
					<a
						href="https://github.com/flamboh/atlantis"
						class="hover:text-foreground"
						target="_blank"
						rel="noopener noreferrer"
					>
						GitHub
					</a>
				</div>
				<div>Built as part of an NSF REU with the Oregon Networking Research Group</div>
				<div>&copy; 2025 Oliver Boorstein &middot; MIT License</div>
			</footer>
		</main>
	</div>
</Tooltip.Provider>
