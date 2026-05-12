{
  description = "A SRT server for v4l2loopback devices";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
    in {
      devShells.${system}.default = pkgs.mkShell {
        packages = with pkgs; [
          rustc
          cargo
          rustfmt
          clippy
          pkg-config
          clang
          llvmPackages.libclang
          ffmpeg
          srt
          libv4l
        ];

        LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
      };

      packages.${system}.default = pkgs.writeShellScriptBin "rstcam-info" ''
        echo "Use: nix develop, then cargo run -- --config config.toml"
      '';
    };
}
