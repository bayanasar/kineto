import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

import '../hook/build.dart' as native_hook;

void main() {
  test('native build cache configuration covers every Cargo manifest and pin',
      () {
    final appRoot = Directory.current.absolute;
    final workspaceRoot = appRoot.parent.parent;
    final expected = <String>{
      File('${workspaceRoot.path}/Cargo.toml').absolute.path,
      File('${workspaceRoot.path}/Cargo.lock').absolute.path,
      File('${workspaceRoot.path}/rust-toolchain.toml').absolute.path,
      File('${appRoot.path}/rust/Cargo.toml').absolute.path,
      File('${appRoot.path}/rust/rust-toolchain.toml').absolute.path,
      ...Directory('${workspaceRoot.path}/crates')
          .listSync(followLinks: false)
          .whereType<Directory>()
          .map((entry) => File('${entry.path}/Cargo.toml'))
          .where((file) => file.existsSync())
          .map((file) => file.absolute.path),
    };

    final inputs = native_hook.nativeBuildConfigurationInputs(
      packageRoot: appRoot,
    );
    final actual = inputs
        .map((uri) => File.fromUri(uri).absolute.path)
        .toSet();

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

  test('rustup setup installs only the requested build target when missing',
      () {
    const requestedTarget = 'x86_64-unknown-linux-gnu';
    final commands = native_hook.rustupSetupArguments(
      channel: '1.98.1',
      targetTriple: requestedTarget,
      toolchainInstalled: false,
      targetInstalled: false,
    );

    expect(commands, hasLength(2));
    expect(commands[1], <String>[
      'target',
      'add',
      requestedTarget,
      '--toolchain',
      '1.98.1',
    ]);
    final flattened = commands.expand((command) => command).toList();
    expect(flattened.where((value) => value == requestedTarget), hasLength(1));
    for (final otherTarget in <String>[
      'aarch64-apple-darwin',
      'x86_64-apple-darwin',
      'aarch64-pc-windows-msvc',
      'x86_64-pc-windows-msvc',
      'aarch64-unknown-linux-gnu',
    ]) {
      expect(flattened, isNot(contains(otherTarget)));
    }
  });

  test('rustup setup is mutation-free when toolchain and target exist', () {
    final commands = native_hook.rustupSetupArguments(
      channel: '1.98.1',
      targetTriple: 'x86_64-unknown-linux-gnu',
      toolchainInstalled: true,
      targetInstalled: true,
    );

    expect(commands, isEmpty);
  });

  test('rustup setup adds only target when toolchain already exists', () {
    final commands = native_hook.rustupSetupArguments(
      channel: '1.98.1',
      targetTriple: 'aarch64-unknown-linux-gnu',
      toolchainInstalled: true,
      targetInstalled: false,
    );

    expect(commands, <List<String>>[
      <String>[
        'target',
        'add',
        'aarch64-unknown-linux-gnu',
        '--toolchain',
        '1.98.1',
      ],
    ]);
  });
}
