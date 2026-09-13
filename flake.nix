{
  description = "goal-app — goal-mode tools, hooks, and TUI extension for rushi";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, fenix }:
    let
      supportedSystems = [ "x86_64-linux" "aarch64-linux" ];
      pkgLib = nixpkgs.lib;

      buildFor = system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ fenix.overlays.default ];
          };
          rustToolchain = fenix.packages.${system}.stable.withComponents [
            "cargo" "clippy" "rust-src" "rustc" "rustfmt" "rust-analyzer"
          ];

          # Build one standalone cargo crate from a subpath of this flake's
          # source tree. Intra-repo path deps (goal-state) resolve because
          # the flake source tree stays intact in the Nix store.
          # The output binary name comes from the crate's [[bin]] name in
          # Cargo.toml, not from crateName.
          buildCrate = { crateDir, crateName }:
            pkgs.rustPlatform.buildRustPackage {
              pname = crateName;
              version = "0.1.0";
              src = "${self}/${crateDir}";
              nativeBuildInputs = [ rustToolchain ];
              cargoLock = { lockFile = "${self}/${crateDir}/Cargo.lock"; };
              doCheck = false;
            };

          # ── Tool wrapper: $out/<name>/tool.toml + <name>/bin/<binary> ──
          wrapAsTool = { name, toolToml, built }:
            pkgs.stdenv.mkDerivation {
              pname = "${name}-tool";
              # stdenv.mkDerivation computes `name` only when both `pname`
              # and `version` are present, so pin a version here.
              version = "0.1.0";
              nativeBuildInputs = [ built ];
              installPhase = ''
                mkdir -p $out/${name}/bin
                cp -rL ${built}/bin/. $out/${name}/bin/
                cp ${toolToml} $out/${name}/tool.toml
              '';
            };

          # ── UI-ext wrapper: $out/<ext>/ext.toml + <ext>/<binDir>/<bin> ──
          # binDir must equal the relative `command` path in ext.toml,
          # because the TUI resolves `command` against the ext entry dir.
          # goal-ext/ext.toml reads `command = "target/release/goal-ext"`,
          # matching the buildRustPackage release layout, so the default
          # binDir applies. Pass a matching binDir only if an ext.toml
          # points at a different dir (e.g. a dev-build target/debug/).
          wrapAsExt = { extName, extToml, built, binDir ? "target/release" }:
            pkgs.stdenv.mkDerivation {
              pname = "${extName}-ui-ext";
              # stdenv.mkDerivation computes `name` only when both `pname`
              # and `version` are present, so pin a version here.
              version = "0.1.0";
              nativeBuildInputs = [ built ];
              installPhase = ''
                mkdir -p $out/${extName}/${binDir}
                cp ${extToml} $out/${extName}/ext.toml
                cp -rL ${built}/bin/. $out/${extName}/${binDir}/
              '';
            };
        in
        # Per-system package attrset.
        rec {
          # ── Tools (wrap the cargo build into the tool contract) ──
          goal          = wrapAsTool { name = "goal";          toolToml = "${self}/goal-tools/goal/tool.toml";          built = buildCrate { crateDir = "goal-tools/goal";         crateName = "goal"; }; };
          goal-blocked  = wrapAsTool { name = "goal_blocked";  toolToml = "${self}/goal-tools/goal_blocked/tool.toml";  built = buildCrate { crateDir = "goal-tools/goal_blocked";  crateName = "goal_blocked"; }; };
          goal-complete = wrapAsTool { name = "goal_complete"; toolToml = "${self}/goal-tools/goal_complete/tool.toml"; built = buildCrate { crateDir = "goal-tools/goal_complete"; crateName = "goal_complete"; }; };

          # ── Hooks (a bare buildRustPackage result IS the hook source) ──
          # Binary names (harness-hook-*) come from each Cargo.toml [[bin]] name;
          # they must match the `command` in config.toml [hooks].
          hook-goal-idle     = buildCrate { crateDir = "goal-hooks/hook-goal-idle";     crateName = "hook-goal-idle"; };
          hook-goal-compact  = buildCrate { crateDir = "goal-hooks/hook-goal-compact";  crateName = "hook-goal-compact"; };
          hook-goal-tools    = buildCrate { crateDir = "goal-hooks/hook-goal-tools";    crateName = "hook-goal-tools"; };
          hook-goal-arm      = buildCrate { crateDir = "goal-hooks/hook-goal-arm";      crateName = "hook-goal-arm"; };
          hook-goal-tokens   = buildCrate { crateDir = "goal-hooks/hook-goal-tokens";   crateName = "hook-goal-tokens"; };

          # ── UI extension (wrap into the ext contract) ──
          # extName "goal" matches the ui_extensions/goal entry name.
          # ext.toml `command = "target/release/goal-ext"` matches the
          # buildRustPackage release layout, so the default binDir applies.
          goal-ext = wrapAsExt {
            extName = "goal";
            extToml = "${self}/goal-ext/ext.toml";
            built = buildCrate { crateDir = "goal-ext"; crateName = "goal-ext"; };
          };

          default = goal;
        };
    in
    {
      # Top-level packages (system as the inner key).
      # `nix build .#<name>` resolves packages.<host>.<name>.
      # A consumer reads goalAppFlake.packages.<system>.<name>.
      packages = pkgLib.genAttrs supportedSystems (system: buildFor system);

      # Dev shell for in-tree cargo work (mirrors rushi-tui precedent).
      devShells = pkgLib.genAttrs supportedSystems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ fenix.overlays.default ];
          };
          rustToolchain = fenix.packages.${system}.stable.withComponents [
            "cargo" "clippy" "rust-src" "rustc" "rustfmt" "rust-analyzer"
          ];
        in
        {
          default = pkgs.mkShell {
            buildInputs = [ rustToolchain pkgs.jq ];
            shellHook = ''
              echo "goal-app dev shell: rust toolchain on PATH."
              echo "Build all crates: cargo build in each crate dir."
            '';
          };
        });
    };
}
