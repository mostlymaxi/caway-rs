{
  lib,
  rustPlatform,
  makeBinaryWrapper,
  cava,
}:

let
  cargo = lib.importTOML ./Cargo.toml;
in
rustPlatform.buildRustPackage {
  pname = cargo.package.name;
  inherit (cargo.package) version;

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./src
    ];
  };

  cargoLock.lockFile = ./Cargo.lock;

  # A compiled wrapper, so no shell process is spawned at startup.
  nativeBuildInputs = [ makeBinaryWrapper ];
  postInstall = ''
    wrapProgram $out/bin/caway --prefix PATH : ${lib.makeBinPath [ cava ]}
  '';

  meta = {
    inherit (cargo.package) description;
    mainProgram = "caway";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
  };
}
