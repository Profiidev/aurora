{
  lib,
  rustPlatform,
  pkg-config,
  fontconfig,
  freetype,
  libxkbcommon,
  libxcb,
  wayland,
  vulkan-loader,
}:

let
  # gpui dlopens these
  gpuiLibs = [
    wayland
    vulkan-loader
    libxkbcommon
  ];
in

rustPlatform.buildRustPackage {
  pname = "aurora";
  version = (lib.importTOML ../../Cargo.toml).package.version;

  src = ./../..;

  cargoLock = {
    lockFile = ../../Cargo.lock;
    # gpui-mobile is only used on Android/iOS, but the lockfile still vendors it
    outputHashes = {
      "gpui-pre-mobile-0.1.0" = "sha256-PWxp1t8Q6cZ0TQKxaS67zS2hNFyJcsmWbgnJjcPzZQM=";
    };
  };

  cargoBuildFlags = [
    "-p"
    "aurora"
    "-p"
    "aurora_daemon"
    "-p"
    "aurora_gui"
  ];

  nativeBuildInputs = [ pkg-config ];

  buildInputs = [
    fontconfig
    freetype
    libxkbcommon
    libxcb
  ];

  # the gpui tests may open windows, CI runs them (tests.yml)
  doCheck = false;

  postInstall = ''
    install -Dm644 platform/packaging/icons/32x32.png $out/share/icons/hicolor/32x32/apps/aurora.png
    install -Dm644 platform/packaging/icons/128x128.png $out/share/icons/hicolor/128x128/apps/aurora.png
    install -Dm644 platform/packaging/icons/128x128@2x.png $out/share/icons/hicolor/256x256/apps/aurora.png
    install -Dm644 platform/packaging/app.desktop $out/share/applications/aurora.desktop
    substituteInPlace $out/share/applications/aurora.desktop \
      --replace-fail "{{categories}}" "Utility;" \
      --replace-fail "{{exec}}" "$out/bin/aurora-gui" \
      --replace-fail "{{icon}}" "aurora" \
      --replace-fail "{{name}}" "Aurora"
    sed -i '/{{/d' $out/share/applications/aurora.desktop
    install -Dm644 platform/packaging/aurorad.service $out/lib/systemd/user/aurorad.service
    substituteInPlace $out/lib/systemd/user/aurorad.service --replace-fail "@bin@" "$out/bin/aurorad"
  '';

  postFixup = ''
    patchelf --add-rpath ${lib.makeLibraryPath gpuiLibs} $out/bin/aurora-gui
  '';

  meta = {
    description = "Aurora";
    homepage = "https://github.com/Profiidev/aurora";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
    mainProgram = "aurora";
  };
}
