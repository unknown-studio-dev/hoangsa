.PHONY: ui ui-clean

# Build the React SPA into crates/hoangsa-ui-web/dist/ for local cargo runs.
# dist/ is gitignored; the release workflow rebuilds it on each matrix runner
# before `cargo build` so the embedded asset set isn't empty.
ui:
	cd crates/hoangsa-ui-web && npm install --silent && npm run build

ui-clean:
	rm -rf crates/hoangsa-ui-web/dist/* crates/hoangsa-ui-web/node_modules
