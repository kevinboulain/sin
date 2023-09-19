{
  inputs.nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
  outputs = { nixpkgs, ... }:
    let
      forSystem = system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [
              (self: super: {
                dovecot = super.dovecot.overrideAttrs (old: {
                  doCheck = false;
                  # Remove most patches, especially 2.3.x-module_dir.patch which
                  # hardcodes /etc/dovecot/modules and makes it impossible to
                  # start dovecot as non-root...
                  # https://github.com/NixOS/nixpkgs/issues/158182
                  patches = [
                    (super.fetchpatch {
                      url = "https://salsa.debian.org/debian/dovecot/-/raw/debian/1%252.3.19.1+dfsg1-2/debian/patches/Support-openssl-3.0.patch";
                      hash = "sha256-PbBB1jIY3jIC8Js1NY93zkV0gISGUq7Nc67Ul5tN7sw=";
                    })
                  ];
                });
              })
            ];
          };
          toml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
          nativeBuildInputs = with pkgs; [
            # https://gist.github.com/yihuang/b874efb97e99d4b6d12bf039f98ae31e?permalink_comment_id=4311076#gistcomment-4311076
            rustPlatform.bindgenHook
          ];
          buildInputs = with pkgs; [
            (assert builtins.compareVersions notmuch.version "0.38" >= 0; notmuch)
          ];
        in
          {
            packages.${system}.default = pkgs.rustPlatform.buildRustPackage {
              pname = toml.package.name;
              version = toml.package.version;
              cargoLock.lockFile = ./Cargo.lock;
              src = pkgs.lib.cleanSource ./.;
              inherit nativeBuildInputs buildInputs;
              # Tests require a Notmuch configuration and a Dovecot without
              # nixpkgs patches.
              doCheck = false;
            };
            devShells.${system}.default = pkgs.mkShell {
              name = toml.package.name;
              RUST_BACKTRACE = 1;
              shellHook = ''
                imap-plain-pass() {
                  # imap-plain-pass user password-store-entry
                  { echo -ne "\0''${1?}\0" && echo -n "$(pass "''${2?}" | head -1)"; } | base64 --wrap=0
                  echo
                }
                imap-shell() {
                  # imap-shell ssl:host:993
                  # imap-shell tcp:host:143
                  socat readline,crlf "''${1?}"
                }
              '';
              nativeBuildInputs = with pkgs; nativeBuildInputs ++ [
                # Toolchain.
                cargo
                clippy
                rust-analyzer
                rustc  # For rustc --print sysroot (RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc doesn't seem necessary).
                rustfmt
                # Goodies.
                cargo-edit
                cargo-expand
                cargo-flamegraph
                cargo-tarpaulin
              ];
              buildInputs = with pkgs; buildInputs ++ [
                # Tests.
                dovecot
                # Debugging.
                gdb
                socat
              ];
            };
          };
    in
      forSystem "x86_64-linux";
}
