<script lang="ts">
	import { resolve } from '$app/paths';
	import '../app.css';
	import { onMount } from 'svelte';
	import { Sun, Moon } from '@lucide/svelte';
	import { Button } from '$lib/components/ui/button';
	import * as Tooltip from '$lib/components/ui/tooltip';
	import { theme } from '$lib/stores/theme.svelte';

	let { children } = $props();

	onMount(() => {
		theme.syncFromDom();
	});
</script>

<!-- Single app-wide provider required by every Tooltip.Root (bits-ui) -->
<Tooltip.Provider>
	<div class="font-body bg-background text-foreground flex h-dvh flex-col overflow-hidden">
		<header class="border-border bg-card shrink-0 border-b">
			<div class="mx-auto max-w-[95vw] px-4 sm:px-2 lg:px-4">
				<div class="flex items-center justify-between py-4">
					<div>
						<h1 class="text-foreground text-3xl font-bold hover:underline">
							<a href={resolve('/')}>ATLANTIS</a>
						</h1>
					</div>
					<div class="flex items-center gap-4">
						<a href={resolve('/')} class="text-muted-foreground hover:text-foreground">Home</a>
						<a href={resolve('/netflow/files')} class="text-muted-foreground hover:text-foreground"
							>Files</a
						>
						<Button
							variant="ghost"
							size="icon-sm"
							onclick={() => theme.toggle()}
							class="text-muted-foreground hover:text-foreground"
							aria-label={theme.dark ? 'Switch to light mode' : 'Switch to dark mode'}
						>
							{#if theme.dark}
								<Sun size={20} />
							{:else}
								<Moon size={20} />
							{/if}
						</Button>
					</div>
				</div>
			</div>
		</header>

		<main class="app-shell__main min-h-0 flex-1 overflow-y-auto">
			{@render children()}
			<footer
				class="text-muted-foreground flex flex-col items-center justify-center gap-1 py-8 text-[10px]"
			>
				<div class="flex flex-wrap items-center justify-center gap-x-2 text-[14px]">
					<a
						href="https://onrg.gitlab.io"
						class="hover:underline"
						target="_blank"
						rel="noopener noreferrer"
					>
						ONRG
					</a>
					<span>&middot;</span>
					<a
						href="https://github.com/flamboh/atlantis"
						class="hover:underline"
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
