import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

import '../hook/build.dart' as native_hook;

void main() {
  test('native build cache inputs cover every Cargo manifest and toolchain pin',
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

    final inputs = native_hook.nativeBuildInputs(packageRoot: appRoot);
    final actual = inputs
        .map((uri) => File.fromUri(uri).absolute.path)
        .toSet();

    expect(actual, expected);
    expect(inputs.length, actual.length, reason: 'cache inputs must be unique');
  });
}
