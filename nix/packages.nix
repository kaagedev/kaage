{
  pkgs,
  lib,
  rustToolchain,
}:
let
  src = lib.cleanSourceWith {
    src = ../.;
    filter =
      path: type:
      let
        base = baseNameOf path;
      in
      !(lib.elem base [
        "target"
        "dist"
        "result"
        ".git"
        ".direnv"
      ]);
  };

  cargoLockFile = ../Cargo.lock;
  cargoLockData = builtins.fromTOML (builtins.readFile cargoLockFile);

  wasmBindgenPkg = lib.findFirst (p: p.name == "wasm-bindgen") (throw "wasm-bindgen not found in Cargo.lock") cargoLockData.package;
  wasmBindgenAttr = "wasm-bindgen-cli_" + lib.replaceStrings [ "." ] [ "_" ] wasmBindgenPkg.version;
  wasmBindgenCli = pkgs.${wasmBindgenAttr} or pkgs.wasm-bindgen-cli;

  rustPlatform = pkgs.makeRustPlatform {
    cargo = rustToolchain;
    rustc = rustToolchain;
  };

  kaaged = rustPlatform.buildRustPackage {
    pname = "kaaged";
    version = "0.1.0";
    inherit src;

    cargoLock.lockFile = cargoLockFile;

    cargoBuildFlags = [
      "-p"
      "kaaged"
    ];
    cargoTestFlags = [
      "-p"
      "kaage-proto"
      "-p"
      "kaaged"
    ];

    nativeBuildInputs = [ pkgs.pkg-config ];

    meta = with lib; {
      description = "kaage BeOS-style service & terminal webtop host daemon";
      license = licenses.mit;
      mainProgram = "kaaged";
      platforms = platforms.unix;
    };
  };

  kaage-ui = pkgs.stdenv.mkDerivation {
    pname = "kaage-ui";
    version = "0.1.0";
    inherit src;

    cargoDeps = rustPlatform.importCargoLock {
      lockFile = cargoLockFile;
    };

    nativeBuildInputs = [
      rustPlatform.cargoSetupHook
      rustToolchain
      wasmBindgenCli
      pkgs.binaryen
    ];

    buildPhase = ''
      runHook preBuild
      cargo build \
        --offline \
        --release \
        --target wasm32-unknown-unknown \
        -p kaage-ui
      runHook postBuild
    '';

    installPhase = ''
      runHook preInstall
      mkdir -p "$out"
      wasm-bindgen \
        --target web \
        --out-dir "$out" \
        --out-name kaage-ui \
        --no-typescript \
        target/wasm32-unknown-unknown/release/kaage-ui.wasm
      wasm-opt -Oz \
        --enable-bulk-memory \
        --enable-nontrapping-float-to-int \
        --enable-sign-ext \
        --enable-mutable-globals \
        --enable-reference-types \
        -o "$out/kaage-ui_bg.opt.wasm" "$out/kaage-ui_bg.wasm"
      mv "$out/kaage-ui_bg.opt.wasm" "$out/kaage-ui_bg.wasm"
      WASM_HASH=$(sha256sum "$out/kaage-ui_bg.wasm" | cut -c1-12)
      sed "s|<link data-trunk[^>]*>|<script type=\"module\">import init from \"./kaage-ui.js?v=$WASM_HASH\"; init({ module_or_path: \"./kaage-ui_bg.wasm?v=$WASM_HASH\" });</script>|" \
        index.html > "$out/index.html"
      runHook postInstall
    '';

    meta = with lib; {
      description = "kaage BeOS-style WASM webtop frontend bundle";
      license = licenses.mit;
    };
  };

  kaage-ui-native = rustPlatform.buildRustPackage {
    pname = "kaage-ui-native";
    version = "0.1.0";
    inherit src;

    cargoLock.lockFile = cargoLockFile;

    cargoBuildFlags = [
      "-p"
      "kaage-ui"
    ];
    doCheck = false;

    nativeBuildInputs = [
      pkgs.pkg-config
      pkgs.makeWrapper
    ];

    buildInputs = lib.optionals pkgs.stdenv.isLinux (
      with pkgs;
      [
        wayland
        libxkbcommon
        vulkan-loader
        libGL
        xorg.libX11
        xorg.libXcursor
        xorg.libXi
        xorg.libXrandr
      ]
    );

    postFixup = lib.optionalString pkgs.stdenv.isLinux ''
      wrapProgram "$out/bin/kaage-ui" \
        --prefix LD_LIBRARY_PATH : "${
          lib.makeLibraryPath (
            with pkgs;
            [
              wayland
              libxkbcommon
              vulkan-loader
              libGL
              xorg.libX11
              xorg.libXcursor
              xorg.libXi
              xorg.libXrandr
            ]
          )
        }"
    '';

    meta = with lib; {
      description = "kaage native desktop client";
      license = licenses.mit;
      mainProgram = "kaage-ui";
    };
  };

  kaage = pkgs.symlinkJoin {
    name = "kaage-0.1.0";
    paths = [ kaaged ];
    nativeBuildInputs = [ pkgs.makeWrapper ];
    postBuild = ''
      wrapProgram "$out/bin/kaaged" \
        --set-default KAAGE_DIST_DIR "${kaage-ui}" \
        --suffix PATH : "${
          lib.makeBinPath (
            [
              pkgs.bashInteractive
              pkgs.coreutils
              pkgs.gnutar
              pkgs.gzip
            ]
            ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.iproute2 ]
          )
        }"
    '';
    meta = kaaged.meta;
  };
in
{
  inherit
    kaaged
    kaage-ui
    kaage-ui-native
    kaage
    wasmBindgenCli
    ;
  default = kaage;
}
