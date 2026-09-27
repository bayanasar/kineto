import 'dart:io';

import 'package:hooks/hooks.dart';
import 'package:native_toolchain_rust/native_toolchain_rust.dart';

List<Uri> nativeBuildInputs({Directory? packageRoot}) {
  final appRoot = (packageRoot ?? Directory.current).absolute;
  final workspaceRoot = appRoot.parent.parent;
  final crateManifests = Directory('${workspaceRoot.path}/crates')
      .listSync(followLinks: false)
      .whereType<Directory>()
      .map((entry) => File('${entry.path}/Cargo.toml'))
      .where((file) => file.existsSync())
      .toList()
    ..sort((left, right) => left.path.compareTo(right.path));

  return <File>[
    File('${workspaceRoot.path}/Cargo.toml'),
    File('${workspaceRoot.path}/Cargo.lock'),
    File('${workspaceRoot.path}/rust-toolchain.toml'),
    ...crateManifests,
    File('${appRoot.path}/rust/Cargo.toml'),
    File('${appRoot.path}/rust/rust-toolchain.toml'),
  ].map((file) => file.absolute.uri).toList(growable: false);
}

void main(List<String> args) async {
  await build(args, (input, output) async {
    final ownershipTestHooks = input.userDefines['ownership_test_hooks'];
    if (ownershipTestHooks is! bool?) {
      throw const FormatException(
        'hooks.user_defines.kineto.ownership_test_hooks must be a boolean',
      );
    }

    // Cargo manifests, lock data, and toolchain pins are build inputs just as
    // much as Rust sources are. Enumerating crates keeps the hook cache
    // correct as the workspace grows instead of relying on a hand-maintained
    // manifest allowlist.
    output.dependencies.addAll(nativeBuildInputs());

    await RustBuilder(
      // Match lib/native/kineto_engine.dart so @Native resolves this CodeAsset
      // by its library URI with no platform-specific library lookup code.
      assetName: 'native/kineto_engine.dart',
      features: <String>[
        if (ownershipTestHooks == true) 'ownership-test-hooks',
      ],
    ).run(input: input, output: output);
  });
}
