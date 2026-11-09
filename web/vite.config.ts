import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// During `npm run dev`, API and WebSocket calls go to the Rust server (run it with --http or keep HTTPS).
const backend = process.env.NDS_BACKEND ?? 'https://localhost:8080';

export default defineConfig({
  plugins: [svelte()],
  server: {
    proxy: {
      '/ws': { target: backend, ws: true, secure: false },
      '/api': { target: backend, secure: false },
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
  },
});
