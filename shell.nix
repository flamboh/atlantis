#with (import <nixpkgs> {});
with (import (builtins.fetchTarball {
  name = "nixos-25.05";
  url = "https://github.com/nixos/nixpkgs/archive/ce01daebf8489ba97bd1609d185ea276efdeb121.tar.gz";
  sha256 = "10cqhkqkifcgyibj9nwxrnq424crfl40kwr3daky83m2fisb4f6p";
}) {});
let
  system = stdenv.hostPlatform.system;

  nodeVersion = "24.18.1";
  nodePlatform =
    {
      x86_64-linux = { name = "linux-x64"; hash = "sha256-1sZk3z8/YUWOjCd1hVcTKFItcFFmcjp8eCOpJTpNFaA="; };
      aarch64-linux = { name = "linux-arm64"; hash = "sha256-cgHjoJ3IJbrFeGfIGRPiuPDvh9BMuQgq9M2oL2/z2Iw="; };
      x86_64-darwin = { name = "darwin-x64"; hash = "sha256-+JLHiVcg9A03UL3iTzVUJC028jYCtRZ7W3PsTROTiu8="; };
      aarch64-darwin = { name = "darwin-arm64"; hash = "sha256-HWC3A/5dfnBySJvoGH9DDxoJWmWMMeXh4oEzGlhz+sM="; };
    }
    .${system} or (throw "Unsupported system: ${system}");
  nodejs = stdenv.mkDerivation {
    pname = "nodejs";
    version = nodeVersion;
    src = fetchurl {
      url = "https://nodejs.org/dist/v${nodeVersion}/node-v${nodeVersion}-${nodePlatform.name}.tar.xz";
      inherit (nodePlatform) hash;
    };
    nativeBuildInputs = lib.optionals stdenv.hostPlatform.isLinux [ autoPatchelfHook ];
    buildInputs = lib.optionals stdenv.hostPlatform.isLinux [ stdenv.cc.cc.lib ];
    dontConfigure = true;
    dontBuild = true;
    installPhase = ''
      mkdir -p $out
      cp -r bin include lib share $out/
    '';
  };

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
    nodejs
    pkgs.pkg-config
    pkgs.python3
    pkgs.rustup
    pkgs.playwright-driver.browsers
  ];

  PLAYWRIGHT_BROWSERS_PATH = "${pkgs.playwright-driver.browsers}";
  PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD = "1";
  PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS = "true";
}
