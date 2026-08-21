<script lang="ts">
	import { resolve } from '$app/paths';

	let { datasetId, active }: { datasetId: string; active: 'dashboard' | 'alerts' } = $props();

	const CONTROL_GROUP_CLASS =
		'dark:border-dark-border dark:bg-dark-subtle grid w-full grid-cols-2 gap-0.5 rounded-md border border-gray-200 bg-gray-50 p-1 sm:w-fit';
	const CONTROL_BUTTON_CLASS =
		'flex min-h-7 items-center justify-center rounded px-2.5 py-0.5 text-center text-xs font-medium transition-colors focus:ring-2 focus:ring-blue-500 focus:outline-none';
	const CONTROL_BUTTON_INACTIVE_CLASS =
		'text-gray-700 hover:text-gray-900 dark:text-gray-300 dark:hover:text-gray-100';
	const CONTROL_BUTTON_ACTIVE_CLASS = 'bg-blue-600 text-white shadow-sm';
</script>

<nav aria-label="Dataset views" class={CONTROL_GROUP_CLASS}>
	<a
		href={resolve('/datasets/[dataset]', { dataset: datasetId })}
		class={`${CONTROL_BUTTON_CLASS} ${
			active === 'dashboard' ? CONTROL_BUTTON_ACTIVE_CLASS : CONTROL_BUTTON_INACTIVE_CLASS
		}`}
		aria-current={active === 'dashboard' ? 'page' : undefined}>Dashboard</a
	>
	<a
		href={resolve('/datasets/[dataset]/alerts', { dataset: datasetId })}
		class={`${CONTROL_BUTTON_CLASS} ${
			active === 'alerts' ? CONTROL_BUTTON_ACTIVE_CLASS : CONTROL_BUTTON_INACTIVE_CLASS
		}`}
		aria-current={active === 'alerts' ? 'page' : undefined}>Alerts</a
	>
</nav>
