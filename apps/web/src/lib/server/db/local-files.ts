import { DATABASE_PATH, LOCAL_DATA_DIR, LOCAL_SQLITE_PATH } from '$app/env/private';

async function resolvePath(value: string): Promise<string> {
	if (value === ':memory:') {
		return value;
	}

	const path = await import('node:path');
	return path.isAbsolute(value) ? value : path.resolve(process.cwd(), value);
}

export async function discoverLocalSqlitePaths(): Promise<string[]> {
	const configured = LOCAL_SQLITE_PATH ?? DATABASE_PATH;
	if (configured) {
		return [await resolvePath(configured)];
	}

	const fs = await import('node:fs/promises');
	const path = await import('node:path');
	const roots = LOCAL_DATA_DIR
		? [await resolvePath(LOCAL_DATA_DIR)]
		: [path.resolve(process.cwd(), 'data'), path.resolve(process.cwd(), '../../data')];
	const dbPaths = new Set<string>();

	for (const root of roots) {
		let entries: import('node:fs').Dirent[];
		try {
			entries = await fs.readdir(root, { withFileTypes: true });
		} catch {
			continue;
		}

		for (const entry of entries) {
			if (!entry.isDirectory()) {
				continue;
			}
			const dbPath = path.join(root, entry.name, 'netflow.sqlite');
			try {
				const stat = await fs.stat(dbPath);
				if (stat.isFile()) {
					dbPaths.add(dbPath);
				}
			} catch {
				// Not every data directory is a web dataset.
			}
		}
	}

	return [...dbPaths].sort();
}
