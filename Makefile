.PHONY: ui ui-clean

# Build the React SPA into crates/hoangsa-ui-web/dist/. The dist/ output is
# committed to the repo (locked decision Q2) so `cargo build` and CI don't
# need Node installed — only refresh runs do.
ui:
	cd crates/hoangsa-ui-web && npm install --silent && npm run build

ui-clean:
	rm -rf crates/hoangsa-ui-web/dist/* crates/hoangsa-ui-web/node_modules
