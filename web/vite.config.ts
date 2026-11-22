import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// In development the GUI runs on Vite (5173) and the API on the server (8080).
// Start the server with SWITCHBOARD_ORIGIN=http://localhost:5173 so passkeys work.
export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      "/api": { target: "http://localhost:8080", changeOrigin: false },
    },
  },
});
