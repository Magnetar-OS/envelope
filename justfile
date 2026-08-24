# Name of the application's binary.
name := 'envelope'
# The unique ID of the application.
appid := 'io.github.entro314labs.Envelope'

# Path to root file system, which defaults to `/`.
rootdir := ''
# The prefix for the `/usr` directory.
prefix := '/usr'
# The location of the cargo target directory.
cargo-target-dir := env('CARGO_TARGET_DIR', 'target')

# Sources. Named after the IDs they install as, because desktop-file-validate
# and appstreamcli both check the file name against the component ID — a source
# called `app.desktop` fails validation that the installed copy would pass.
desktop-src := 'resources' / (appid + '.desktop')
metainfo-src := 'resources' / (appid + '.metainfo.xml')
icon-src := 'resources' / 'icons' / 'hicolor' / 'scalable' / 'apps' / 'icon.svg'

# Install destinations
base-dir := absolute_path(clean(rootdir / prefix))
bin-dst := base-dir / 'bin' / name
desktop-dst := base-dir / 'share' / 'applications' / (appid + '.desktop')
appdata-dst := base-dir / 'share' / 'metainfo' / (appid + '.metainfo.xml')
icons-dst := base-dir / 'share' / 'icons' / 'hicolor'
icon-svg-dst := icons-dst / 'scalable' / 'apps' / (appid + '.svg')

# Default recipe which runs `just build-release`
default: build-release

# Everything CI runs, in the order that fails cheapest first
check-all: validate-metadata fmt-check check test

# Runs `cargo clean`
clean:
    cargo clean

# Removes vendored dependencies
clean-vendor:
    rm -rf .cargo vendor vendor.tar

# `cargo clean` and removes vendored dependencies
clean-dist: clean clean-vendor

# Compiles with debug profile
build-debug *args:
    cargo build --locked {{args}}

# Compiles with release profile
build-release *args: (build-debug '--release' args)

# Compiles release profile with vendored dependencies
build-vendored *args: vendor-extract (build-release '--frozen --offline' args)

# Runs a clippy check
check *args:
    cargo clippy --all-features --all-targets --locked {{args}} -- -W clippy::pedantic

# Runs a clippy check with JSON message format
check-json: (check '--message-format=json')

# Checks formatting without rewriting anything
#
# Scoped to this package: `--all` would reach through the path dependency and
# reformat the cosmic-pim checkout, which is a different repository.
fmt-check:
    cargo fmt -p envelope -- --check

# Rewrites formatting
fmt:
    cargo fmt -p envelope

# Runs the test suite
test *args:
    cargo test --locked {{args}}

# Checks the desktop entry and AppStream metadata against their specs
#
# Offline: the metainfo names a remote icon on `main`, which appstreamcli cannot
# fetch from a branch that has not been merged yet.
validate-metadata:
    desktop-file-validate {{desktop-src}}
    appstreamcli validate --no-net {{metainfo-src}}

# Also checks that the remote icon and URLs actually resolve
validate-metadata-urls:
    appstreamcli validate {{metainfo-src}}

# Run the application for testing purposes
run *args:
    env RUST_LOG=envelope=debug RUST_BACKTRACE=full cargo run {{args}}

# Installs the application
install:
    install -Dm0755 {{ cargo-target-dir / 'release' / name }} {{bin-dst}}
    install -Dm0644 {{desktop-src}} {{desktop-dst}}
    install -Dm0644 {{metainfo-src}} {{appdata-dst}}
    install -Dm0644 {{icon-src}} {{icon-svg-dst}}

# Uninstalls installed files
uninstall:
    rm -f {{bin-dst}} {{desktop-dst}} {{appdata-dst}} {{icon-svg-dst}}

# Installs into the current user's home, no root needed. Handy for trying it out.
install-user:
    just rootdir='' prefix={{home_directory()}}/.local install

# Vendor dependencies locally
vendor:
    #!/usr/bin/env bash
    mkdir -p .cargo
    cargo vendor --sync Cargo.toml | head -n -1 > .cargo/config.toml
    echo 'directory = "vendor"' >> .cargo/config.toml
    tar pcf vendor.tar vendor
    rm -rf vendor

# Extracts vendored dependencies
vendor-extract:
    rm -rf vendor
    tar pxf vendor.tar
