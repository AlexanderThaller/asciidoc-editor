{
  description = "A WYSIWYG AsciiDoc editor for the web, in Rust and WebAssembly";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      each = nixpkgs.lib.genAttrs systems;
      pkgsFor = system: nixpkgs.legacyPackages.${system};

      # Everything the package build shells out to. The generator has to be the
      # same version as the wasm-bindgen the crate is compiled against, which is
      # why Cargo.toml pins that one exactly: these two move together or not at
      # all, and build-package.sh refuses to run when they disagree.
      toolsFor =
        pkgs: with pkgs; [
          cargo
          rustc
          clippy
          rustfmt
          wasm-bindgen-cli_0_2_127
          binaryen
          nodejs
        ];
    in
    {
      devShells = each (system:
        let pkgs = pkgsFor system;
        in {
          default = pkgs.mkShell {
            packages = toolsFor pkgs ++ [ pkgs.trunk ];

            shellHook = ''
              echo "rustc      $(rustc --version | cut -d' ' -f2)"
              echo "wasm-bindgen $(wasm-bindgen --version | cut -d' ' -f2)"
              echo "wasm-opt   $(wasm-opt --version | cut -d' ' -f3)"
            '';
          };
        });

      # `nix run .#package` builds pkg/, the directory that gets published.
      packages = each (system:
        let pkgs = pkgsFor system;
        in {
          package = pkgs.writeShellApplication {
            name = "build-package";
            runtimeInputs = toolsFor pkgs;
            text = ''exec ./build-package.sh "$@"'';
          };
        });

      formatter = each (system: (pkgsFor system).nixfmt);
    };
}
