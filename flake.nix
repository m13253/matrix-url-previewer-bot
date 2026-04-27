{
  description = "A Matrix bot that serves URL previews from a trusted server to the whole room";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
    let
      pkgs = import nixpkgs { inherit system; };
    in
    {
      packages = {
        matrix-url-previewer-bot = 
          let
            manifest = (pkgs.lib.importTOML ./Cargo.toml).package;
          in
          pkgs.rustPlatform.buildRustPackage {
            pname = manifest.name;
            version = manifest.version;
          
            cargoLock.lockFile = ./Cargo.lock;
          
            src = pkgs.lib.cleanSource ./.;
          
            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = [
              pkgs.openssl
              pkgs.sqlite
            ];
          };
        default = self.packages.${system}.matrix-url-previewer-bot;
      };
    })
    // {
      overlays.default = final: prev: {
        inherit (self.packages.${final.system}) matrix-url-previewer-bot;
      };
    };
}
