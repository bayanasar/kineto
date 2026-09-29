import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

import '../hook/build.dart' as native_hook;

void main() {
  test('native build cache configuration covers every Cargo manifest and pin',
      () {
    final appRoot = Directory.current.absolute;
    final workspaceRoot = appRoot.parent.parent;
    final expected = <Uri>{
      File('${workspaceRoot.path}/Cargo.toml').absolute.uri,
      File('${workspaceRoot.path}/Cargo.lock').absolute.uri,
      File('${workspaceRoot.path}/rust-toolchain.toml').absolute.uri,
      File('${appRoot.path}/rust/Cargo.toml').absolute.uri,
      File('${appRoot.path}/rust/rust-toolchain.toml').absolute.uri,
      ...Directory('${workspaceRoot.path}/crates')
          .listSync(followLinks: false)
          .whereType<Directory>()
          .map((entry) => File('${entry.path}/Cargo.toml'))
          .where((file) => file.existsSync())
          .map((file) => file.absolute.uri),
    };

    final inputs = native_hook.nativeBuildConfigurationInputs(
      packageRoot: appRoot,
    );
    final actual = inputs.toSet();

    expect(actual, expected);
    expect(inputs.length, actual.length, reason: 'cache inputs must be unique');
  });

  test('native build cache tracks every Rust source in the workspace', () {
    final appRoot = Directory.current.absolute;
    final workspaceRoot = appRoot.parent.parent;
    final expectedSources = <String>{
      ...Directory('${workspaceRoot.path}/crates')
          .listSync(recursive: true, followLinks: false)
          .whereType<File>()
          .where((file) => file.path.endsWith('.rs'))
          .map((file) => file.absolute.path),
      ...Directory('${appRoot.path}/rust')
          .listSync(recursive: true, followLinks: false)
          .whereType<File>()
          .where((file) => file.path.endsWith('.rs'))
          .map((file) => file.absolute.path),
    };

    final inputs = native_hook.nativeBuildInputs(packageRoot: appRoot);
    final actual = inputs
        .map((uri) => File.fromUri(uri).absolute.path)
        .toSet();

    expect(actual, containsAll(expectedSources));
    expect(inputs.length, actual.length, reason: 'cache inputs must be unique');
  });

  test('root and app Rust toolchain pins agree and do not preinstall targets',
      () {
    final appRoot = Directory.current.absolute;
    final workspaceRoot = appRoot.parent.parent;
    final rootToolchain = File('${workspaceRoot.path}/rust-toolchain.toml');
    final appToolchain = File('${appRoot.path}/rust/rust-toolchain.toml');

    expect(
      native_hook.readToolchainChannel(appToolchain),
      native_hook.readToolchainChannel(rootToolchain),
    );
    expect(appToolchain.readAsStringSync(), isNot(contains('targets =')));
  });

  test('installed rust targets are parsed from rustup whitespace output', () {
    expect(
      native_hook.parseInstalledRustTargets(
        'x86_64-unknown-linux-gnu\naarch64-unknown-linux-gnu\n',
      ),
      <String>{
        'x86_64-unknown-linux-gnu',
        'aarch64-unknown-linux-gnu',
      },
    );
  });

  test('rust setup guidance keeps target installation explicit and narrow', () {
    expect(
      native_hook.rustToolchainInstallCommand('1.98.1'),
      'rustup toolchain install 1.98.1 --profile minimal '
      '--component rustfmt --component clippy',
    );
    expect(
      native_hook.rustTargetInstallCommand(
        channel: '1.98.1',
        targetTriple: 'x86_64-unknown-linux-gnu',
      ),
      'rustup target add x86_64-unknown-linux-gnu --toolchain 1.98.1',
    );
  });
}
