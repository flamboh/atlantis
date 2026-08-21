<script lang="ts">
	import { resolve } from '$app/paths';
	import '../app.css';
	import { onMount } from 'svelte';
	import { Sun, Moon } from '@lucide/svelte';
	import { theme } from '$lib/stores/theme.svelte';

	let { children } = $props();

	onMount(() => {
		theme.syncFromDom();
	});
</script>

<div
	class="font-body dark:bg-dark-bg flex h-dvh flex-col overflow-hidden bg-gray-100 text-gray-900 dark:text-gray-100"
>
	<header class="dark:border-dark-border dark:bg-dark-surface shrink-0 border-b bg-white">
		<div class="mx-auto max-w-[95vw] px-4 sm:px-2 lg:px-4">
			<div class="flex items-center justify-between py-4">
				<div>
					<h1 class="text-3xl font-bold text-gray-900 hover:underline dark:text-gray-100">
						<a href={resolve('/')}>ATLANTIS</a>
					</h1>
				</div>
				<div class="flex items-center gap-4">
					<a
						href={resolve('/')}
						class="text-gray-500 hover:text-gray-900 dark:text-gray-400 dark:hover:text-gray-100"
						>Home</a
					>
					<a
						href={resolve('/netflow/files')}
						class="text-gray-500 hover:text-gray-900 dark:text-gray-400 dark:hover:text-gray-100"
						>Files</a
					>
					<button
						type="button"
						onclick={() => theme.toggle()}
						class="dark:hover:bg-dark-subtle rounded-md p-2 text-gray-500 hover:bg-gray-100 hover:text-gray-900 focus:ring-2 focus:ring-blue-500 focus:outline-none dark:text-gray-400 dark:hover:text-gray-100"
						aria-label={theme.dark ? 'Switch to light mode' : 'Switch to dark mode'}
					>
						{#if theme.dark}
							<Sun size={20} />
						{:else}
							<Moon size={20} />
						{/if}
					</button>
				</div>
			</div>
		</div>
	</header>

	<main class="app-shell__main min-h-0 flex-1 overflow-y-auto">
		{@render children()}
		<footer
			class="flex flex-col items-center justify-center gap-1 py-8 text-[10px] text-gray-400 dark:text-gray-500"
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
