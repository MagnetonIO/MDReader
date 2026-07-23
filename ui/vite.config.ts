import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

// https://vite.dev/config/
// Tauri drives this dev server: it expects a fixed port and relies on Vite not
// clearing the screen so Rust build errors stay visible. Mobile host/HMR wiring
// arrives with the mobile targets in P6.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
});
