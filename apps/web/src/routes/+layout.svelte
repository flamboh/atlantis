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

	let { children } = $props();

	onMount(() => {
		theme.syncFromDom();
	});
</script>

<!-- Single app-wide provider required by every Tooltip.Root (bits-ui) -->
<Tooltip.Provider>
	<div class="font-body bg-background text-foreground flex h-dvh flex-col overflow-hidden">
		<header class="border-border bg-card shrink-0 border-b">
			<div class="shell-width">
				<div class="flex items-center justify-between gap-3 py-3">
					<div>
						<h1 class="text-foreground text-lg font-semibold tracking-tight min-[360px]:text-xl">
							<a href={resolve('/')}><Logo /></a>
						</h1>
					</div>
					<nav aria-label="Main navigation" class="flex items-center gap-3 text-sm sm:gap-6">
						<a
							href={resolve('/')}
							aria-current={page.url.pathname === '/' ? 'page' : undefined}
							class="text-muted-foreground hover:text-foreground aria-[current=page]:text-accent-foreground"
							>Home</a
						>
						<a
							href={resolve('/netflow/files')}
							aria-current={page.url.pathname.startsWith('/netflow/files') ? 'page' : undefined}
							class="text-muted-foreground hover:text-foreground aria-[current=page]:text-accent-foreground"
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
					</nav>
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
