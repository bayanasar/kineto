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
  test('rust process environment disables rustup proxy auto-install', () {
    expect(
      native_hook.rustProcessEnvironment(
        parentEnvironment: <String, String>{
          'PATH': '/Applications/Xcode.app/Contents/Developer/usr/bin:/usr/bin',
        },
        isMacOS: true,
      ),
      <String, String>{
        'RUSTUP_AUTO_INSTALL': '0',
        'PATH': '/usr/bin',
      },
    );
    expect(
      native_hook.rustProcessEnvironment(
        parentEnvironment: const <String, String>{'PATH': '/usr/bin'},
        isMacOS: false,
      ),
      <String, String>{'RUSTUP_AUTO_INSTALL': '0'},
    );
  });

  test('reported code asset is copied out of shared intermediates', () async {
    final temp = await Directory.systemTemp.createTemp('kineto-code-asset-');
    addTearDown(() => temp.delete(recursive: true));
    final shared = File('${temp.path}/shared/libkineto_native.so');
    await shared.parent.create(recursive: true);
    await shared.writeAsString('shared-v1');
    final output = Directory('${temp.path}/config-output');

    final copied = await native_hook.copyCodeAssetForConfiguration(
      sharedLibrary: shared,
      outputDirectory: output.uri,
    );

    expect(
      copied.path,
      File.fromUri(output.uri.resolve('libkineto_native.so')).path,
    );
    expect(await copied.readAsString(), 'shared-v1');
    await shared.writeAsString('shared-v2');
    expect(await copied.readAsString(), 'shared-v1');
  });
}
