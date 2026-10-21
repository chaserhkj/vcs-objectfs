{
  lib,
  makeWrapper,
  rustPlatform,
  fuse3,
}:

let
  cargoToml = lib.importTOML ../Cargo.toml;
in
rustPlatform.buildRustPackage {
  pname = "vcs-objectfs";
  version = cargoToml.package.version;

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../src
    ];
  };
  cargoLock.lockFile = ../Cargo.lock;

  nativeBuildInputs = [ makeWrapper ];

  # The `fuser` crate in the dependency tree mounts through its pure-Rust
  # implementation on Linux, which execs `fusermount3` from `PATH`. The runtime
  # dependency is added explicitly, so the store path reaches the binary.
  postFixup = ''
    wrapProgram $out/bin/vcs-objectfs \
      --prefix PATH : ${lib.makeBinPath [ fuse3 ]}
  '';

  # End-to-end tests mount a FUSE filesystem; they run in a private mount
  # namespace, which the build sandbox does not provide. Only the unit tests of
  # the binary target run here.
  cargoTestFlags = [ "--bins" ];

  meta = {
    description = "FUSE mount making VCS object files immutable";
    longDescription = ''
      A FUSE filesystem that treats VCS object files as immutable and passes
      every other file through unchanged. An object file is a regular file
      whose name is a lowercase hexadecimal hash; once a file is renamed to
      such a name it can no longer be written, renamed, removed or have its
      metadata changed.
    '';
    license = lib.licenses.mit;
    mainProgram = "vcs-objectfs";
    platforms = lib.platforms.linux;
  };
}
