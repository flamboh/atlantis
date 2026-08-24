#with (import <nixpkgs> {});
with (import (builtins.fetchTarball {
  name = "nixos-25.05";
  url = "https://github.com/nixos/nixpkgs/archive/ce01daebf8489ba97bd1609d185ea276efdeb121.tar.gz";
  sha256 = "10cqhkqkifcgyibj9nwxrnq424crfl40kwr3daky83m2fisb4f6p";
}) {});
mkShell {
  buildInputs = [
    pkgs.autoconf
    pkgs.automake
    pkgs.bison
    pkgs.bun
    pkgs.flex
    pkgs.gcc
    pkgs.git
    pkgs.gnumake
    pkgs.gnutar
    pkgs.libtool
    pkgs.nodejs
    pkgs.pkg-config
    pkgs.python3
    pkgs.rustup
    pkgs.playwright-driver.browsers
  ];

  PLAYWRIGHT_BROWSERS_PATH = "${pkgs.playwright-driver.browsers}";
  PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD = "1";
  PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS = "true";
}
