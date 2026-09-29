#with (import <nixpkgs> {});
with (import (builtins.fetchTarball {
  name = "nixos-26.05";
  url = "https://github.com/nixos/nixpkgs/archive/02e08985a27c65ffd33d434eeb2e660a2e4dc84d.tar.gz";
  sha256 = "1959piz48qhaaqdyr2m5mf92gnxxhrzhls6z1ppy01ckh5wdrya2";
}) {});
let
  system = stdenv.hostPlatform.system;

  bunVersion = "1.3.11";
  bunPlatform =
    {
      x86_64-linux = { name = "linux-x64"; hash = "sha256-hhG6k1r4hvBabzh0ChUWAybBXl1dB63vlmEwtEk2B+0="; };
      aarch64-linux = { name = "linux-aarch64"; hash = "sha256-0TlE2hKlPsx0v2pyC9HQTEVVwDjf5CI2U1anvkdpH98="; };
      x86_64-darwin = { name = "darwin-x64"; hash = "sha256-xP4rkkchiwKV8k6JWq7I/uYudEUmeakCa2fqy9YRooY="; };
      aarch64-darwin = { name = "darwin-aarch64"; hash = "sha256-b1o0Z+2crsR5W/eM1HZQfZ+HDH1XuGyUX8szgSZ3L/w="; };
    }
    .${system} or (throw "Unsupported system: ${system}");
  bun = pkgs.bun.overrideAttrs {
    version = bunVersion;
    src = fetchurl {
      url = "https://github.com/oven-sh/bun/releases/download/bun-v${bunVersion}/bun-${bunPlatform.name}.zip";
      inherit (bunPlatform) hash;
    };
    sourceRoot = "bun-${bunPlatform.name}";
  };
in
mkShell {
  buildInputs = [
    pkgs.autoconf
    pkgs.automake
    pkgs.bison
    bun
    pkgs.flex
    pkgs.gcc
    pkgs.git
    pkgs.gnumake
    pkgs.gnutar
    pkgs.libtool
    pkgs.nodejs_24
    pkgs.pkg-config
    pkgs.python3
    pkgs.rustup
    pkgs.playwright-driver.browsers
  ];

  PLAYWRIGHT_BROWSERS_PATH = "${pkgs.playwright-driver.browsers}";
  PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD = "1";
  PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS = "true";
}
