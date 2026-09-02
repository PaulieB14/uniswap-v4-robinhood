# `substreams pack` does NOT compile. It packages whatever .wasm already sits at
# the path in substreams.yaml's `binaries:` stanza, with no staleness check and
# no warning. Editing a .rs file and running `substreams pack` therefore produces
# a package whose source and binary disagree — v0.1.0 shipped that way, with
# three commits' worth of fixes present in git and absent from the wasm, so the
# published module still dropped every pool_stats row. `cargo test` did not
# catch it and could not have: it builds for the HOST, so a green suite says
# nothing about the wasm that ships.
#
# Nor is `substreams build` the answer here. It runs a protobuf codegen step
# that writes src/pb/mod.rs, which collides with this package's checked-in
# src/pb.rs (E0761, "file for module `pb` found at both"). Compile with cargo
# and pack separately.

WASM   := target/wasm32-unknown-unknown/release/uniswap_v4_robinhood.wasm
SPKG   := uniswap-v4-robinhood-v0.1.2.spkg
# Only what cargo actually compiles into the wasm. substreams.yaml is read by
# `substreams pack`, not by cargo, so a manifest-only edit (a version bump)
# does NOT make the wasm stale — flagging it would train people to ignore
# this check, which is the one failure it exists to prevent.
SOURCES = src Cargo.toml Cargo.lock

.PHONY: build test pack check publish stale clean-codegen

build: clean-codegen                ## compile to wasm, then pack — the only safe sequence
	cargo build --target wasm32-unknown-unknown --release
	substreams pack

pack: build                         ## alias; never packs without compiling first

test:
	cargo test

stale:                              ## report any source newer than the built wasm
	@if [ ! -f $(WASM) ]; then echo "no wasm built yet"; exit 0; fi; \
	newer=$$(find $(SOURCES) -newer $(WASM) -type f 2>/dev/null | head -5); \
	if [ -n "$$newer" ]; then \
	  echo "WASM IS STALE — do not pack. Newer sources:"; echo "$$newer" | sed 's/^/  /'; \
	  exit 1; \
	else echo "wasm is current"; fi

clean-codegen:                      ## remove substreams-build codegen that shadows src/pb.rs
	@rm -rf src/pb

check: stale-report test build      ## run before publishing
	@echo "OK: tests pass, wasm rebuilt from current source, package packed"

stale-report:                       ## non-fatal staleness note before building
	@$(MAKE) --no-print-directory stale || true

publish: check
	substreams registry publish ./$(SPKG)
