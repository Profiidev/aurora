{
  lib,
  rustPlatform,
  pkg-config,
  fontconfig,
  freetype,
  libxkbcommon,
  libxcb,
  dbus,
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
  version = (lib.importTOML ../Cargo.toml).workspace.package.version;

  src = ./..;

  cargoLock = {
    lockFile = ../Cargo.lock;
    # gpui-mobile is only used on Android/iOS, but the lockfile still vendors it
    outputHashes = {
      "gpui-pre-mobile-0.1.0" = "sha256-0WAb3pWoWiccTe/SwUBkfEAzrHiyElRhGaK8ug2xowk=";
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
    dbus
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
    install -Dm644 platform/packaging/70-aurora.rules $out/lib/udev/rules.d/70-aurora.rules
  '';

  postFixup = ''
    patchelf --add-rpath ${lib.makeLibraryPath gpuiLibs} $out/bin/aurora-gui
  '';

  meta = {
    description = "Aurora";
    homepage = "https://github.com/Profiidev/aurora";
    license = lib.licenses.gpl2;
    maintainers = with lib.maintainers; [ profidev ];
    platforms = lib.platforms.linux;
    mainProgram = "aurora";
  };
}
