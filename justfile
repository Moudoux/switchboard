# Development tasks. `just --list` shows them.

# Run the server for development, with the GUI expected on Vite (:5173)
server:
    cd server && SWITCHBOARD_ORIGIN=http://localhost:5173 SWITCHBOARD_DB=../data/switchboard.db SWITCHBOARD_WEB_DIR=../web/dist cargo run

# Run the GUI with hot reload on :5173
web:
    cd web && pnpm install && pnpm dev

# Print a one-time link to register a passkey on the development server
enroll:
    cd server && SWITCHBOARD_ORIGIN=http://localhost:5173 SWITCHBOARD_DB=../data/switchboard.db cargo run -- enroll

# Formatting, lints, tests and the GUI build: what CI runs
check:
    cd server && cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked
    cd web && pnpm install --frozen-lockfile && pnpm build

# Build the multi-arch image locally (not pushed)
image:
    docker buildx build --platform linux/amd64,linux/arm64 -t switchboard:dev .
