{
  lib,
  stdenv,
  fetchurl,
  autoPatchelfHook,
  zlib,
}:

let
  version = "0.0.211";

  targets = {
    "aarch64-darwin" = "aarch64-apple-darwin";
    "aarch64-linux" = "aarch64-unknown-linux-gnu";
    "x86_64-linux" = "x86_64-unknown-linux-gnu";
  };

  hashes = {
    "aarch64-darwin" = "sha256-YVlQp+QBQ6XeU8unC8M/uHFbF6EWmcQU4qgZC2Rp/7M=";
    "aarch64-linux" = "sha256-JMegK9ivB2ia1mj2trZIyaUnBNHe9RqYbeFauR/3UcQ=";
    "x86_64-linux" = "sha256-ztKyTZRrjCLUIXBCrrdBDHY6eLtE8fzwk0weBzFMCvk=";
  };

  system = stdenv.hostPlatform.system;
  target = lib.attrByPath [ system ] (throw "btmux is not available on ${system}") targets;
  hash = lib.attrByPath [ system ] (throw "no btmux hash for ${system}") hashes;
in
stdenv.mkDerivation {
  pname = "btmux";
  inherit version;

  src = fetchurl {
    url = "https://github.com/btmux-dev/btmux/releases/download/v${version}/btmux-${target}";
    inherit hash;
  };

  dontUnpack = true;
  dontConfigure = true;
  dontBuild = true;
  dontCheck = true;

  nativeBuildInputs = lib.optional stdenv.hostPlatform.isLinux autoPatchelfHook;
  buildInputs = lib.optionals stdenv.hostPlatform.isLinux [
    stdenv.cc.cc.lib
    zlib
  ];

  installPhase = ''
    install -Dm755 "$src" "$out/bin/btmux"
  '';

  meta = {
    description = "Browser-based terminal manager";
    homepage = "https://github.com/btmux-dev/btmux";
    license = lib.licenses.mit;
    mainProgram = "btmux";
    platforms = lib.attrNames targets;
  };
}
