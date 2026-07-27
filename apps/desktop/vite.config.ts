import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// https://tauri.app/v1/guides/getting-started/setup/vite
export default defineConfig(async () => ({
  plugins: [react()],
  clearScreen: false,
  // Preserva symlinks/junctions para que o build funcione quando o
  // repositório é acessado via atalho sem espaços (ex: C:\src\Frederico
  // apontando para um caminho com espaços). Necessário porque o Rollup
  // rejeita paths emitidos que contenham espaço.
  resolve: { preserveSymlinks: true },
  server: {
    port: 1420,
    strictPort: true,
    host: "0.0.0.0",
    hmr: { protocol: "ws", host: "localhost", port: 1421 },
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "chrome105",
    minify: !process.env.TAURI_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_DEBUG,
  },
}));
