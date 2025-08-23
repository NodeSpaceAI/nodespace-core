// See https://kit.svelte.dev/docs/types#app
// for information about these interfaces
declare global {
	namespace App {
		// interface Error {}
		// interface Locals {}
		// interface PageData {}
		// interface PageState {}
		// interface Platform {}
	}
}

// Import meta environment types
interface ImportMetaEnv {
	readonly DEV: boolean;
	readonly TEST?: boolean;
	readonly PROD: boolean;
	readonly MODE: string;
	readonly BASE_URL: string;
}

// eslint-disable-next-line @typescript-eslint/no-unused-vars
interface ImportMeta {
	readonly env: ImportMetaEnv;
}

export {};