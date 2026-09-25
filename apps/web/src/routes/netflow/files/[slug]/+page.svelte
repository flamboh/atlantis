<script lang="ts">
	import type { PageProps } from './$types';
	import NetflowFileDetailPage from '#lib/components/netflow/NetflowFileDetailPage.svelte';

	let { data }: PageProps = $props();
</script>

<NetflowFileDetailPage {data} />
