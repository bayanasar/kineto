import 'dart:io';

import 'package:code_assets/code_assets.dart';
import 'package:hooks/hooks.dart';

const _assetName = 'native/kineto_engine.dart';
const _crateName = 'kineto-native';
const _libraryName = 'kineto_native';

List<Uri> nativeBuildConfigurationInputs({Directory? packageRoot}) {
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

List<Uri> nativeBuildInputs({Directory? packageRoot}) {
  final appRoot = (packageRoot ?? Directory.current).absolute;
  final workspaceRoot = appRoot.parent.parent;
  final sourceFiles = <File>[
    ...Directory('${workspaceRoot.path}/crates')
        .listSync(recursive: true, followLinks: false)
        .whereType<File>()
        .where((file) => file.path.endsWith('.rs')),
    ...Directory('${appRoot.path}/rust')
        .listSync(recursive: true, followLinks: false)
        .whereType<File>()
        .where((file) => file.path.endsWith('.rs')),
  ]..sort((left, right) => left.path.compareTo(right.path));

  final inputs = <Uri>{
    ...nativeBuildConfigurationInputs(packageRoot: appRoot),
    ...sourceFiles.map((file) => file.absolute.uri),
  };
  return inputs.toList(growable: false);
}

String readToolchainChannel(File toolchainFile) {
  final match = RegExp(
    r'^\s*channel\s*=\s*"([^"]+)"\s*$',
    multiLine: true,
  ).firstMatch(toolchainFile.readAsStringSync());
  if (match == null) {
    throw FormatException(
      'missing pinned toolchain.channel in ${toolchainFile.path}',
    );
  }
  return match.group(1)!;
}

String rustToolchainInstallCommand(String channel) =>
    'rustup toolchain install $channel --profile minimal '
    '--component rustfmt --component clippy';

String rustTargetInstallCommand({
  required String channel,
  required String targetTriple,
}) => 'rustup target add $targetTriple --toolchain $channel';
Future<ProcessResult> _runCapture(
  String executable,
  List<String> arguments, {
  required String workingDirectory,
  Map<String, String>? environment,
}) async {
  try {
    return await Process.run(
      executable,
      arguments,
      workingDirectory: workingDirectory,
      environment: environment,
      includeParentEnvironment: true,
      runInShell: Platform.isWindows,
    );
  } on ProcessException catch (error) {
    final path = Platform.environment['PATH'] ?? '<unset>';
    throw StateError(
      'required executable "$executable" was not found or could not start '
      'for the build hook (PATH=$path): $error',
    );
  }
}

Future<void> _run(
  String executable,
  List<String> arguments, {
  required String workingDirectory,
  Map<String, String>? environment,
}) async {
  final result = await _runCapture(
    executable,
    arguments,
    workingDirectory: workingDirectory,
    environment: environment,
  );
  if ((result.stdout as String).isNotEmpty) {
    stdout.write(result.stdout);
  }
  if ((result.stderr as String).isNotEmpty) {
    stderr.write(result.stderr);
  }
  if (result.exitCode != 0) {
    throw ProcessException(
      executable,
      arguments,
      'command failed with exit code ${result.exitCode}',
      result.exitCode,
    );
  }
}

Future<bool> _rustToolchainInstalled({
  required String channel,
  required String workingDirectory,
}) async {
  final result = await _runCapture(
    'rustup',
    <String>['run', channel, 'rustc', '--version'],
    workingDirectory: workingDirectory,
  );
  return result.exitCode == 0;
}

Set<String> parseInstalledRustTargets(String output) => output
    .split(RegExp(r'\s+'))
    .where((entry) => entry.isNotEmpty)
    .toSet();

Future<bool> _rustTargetInstalled({
  required String channel,
  required String targetTriple,
  required String workingDirectory,
}) async {
  final result = await _runCapture(
    'rustup',
    <String>['target', 'list', '--installed', '--toolchain', channel],
    workingDirectory: workingDirectory,
  );
  if (result.exitCode != 0) {
    return false;
  }
  final targets = parseInstalledRustTargets(result.stdout as String);
  return targets.contains(targetTriple);
}

Map<String, String>? _cargoEnvironment() {
  if (!Platform.isMacOS) {
    return null;
  }
  final path = Platform.environment['PATH'];
  if (path == null) {
    return null;
  }
  // Xcode injects developer-tool shims ahead of the host toolchain. Remove
  // those entries so Cargo resolves the normal linker/toolchain consistently.
  return <String, String>{
    'PATH': path
        .split(':')
        .where((entry) => !entry.contains('Contents/Developer/'))
        .join(':'),
  };
}

String targetTripleFor(CodeConfig codeConfig) {
  return switch ((codeConfig.targetOS, codeConfig.targetArchitecture)) {
    (OS.android, Architecture.arm64) => 'aarch64-linux-android',
    (OS.android, Architecture.arm) => 'armv7-linux-androideabi',
    (OS.android, Architecture.x64) => 'x86_64-linux-android',
    (OS.iOS, Architecture.arm64)
        when codeConfig.iOS.targetSdk == IOSSdk.iPhoneSimulator =>
      'aarch64-apple-ios-sim',
    (OS.iOS, Architecture.arm64)
        when codeConfig.iOS.targetSdk == IOSSdk.iPhoneOS =>
      'aarch64-apple-ios',
    (OS.iOS, Architecture.x64) => 'x86_64-apple-ios',
    (OS.windows, Architecture.arm64) => 'aarch64-pc-windows-msvc',
    (OS.windows, Architecture.x64) => 'x86_64-pc-windows-msvc',
    (OS.linux, Architecture.arm64) => 'aarch64-unknown-linux-gnu',
    (OS.linux, Architecture.x64) => 'x86_64-unknown-linux-gnu',
    (OS.macOS, Architecture.arm64) => 'aarch64-apple-darwin',
    (OS.macOS, Architecture.x64) => 'x86_64-apple-darwin',
    (_, _) => throw UnsupportedError(
      'Unsupported Rust target: ${codeConfig.targetOS} '
      'on ${codeConfig.targetArchitecture}',
    ),
  };
}

LinkMode linkModeFor(CodeConfig codeConfig) {
  return switch (codeConfig.linkModePreference) {
    LinkModePreference.dynamic ||
    LinkModePreference.preferDynamic => DynamicLoadingBundled(),
    LinkModePreference.static ||
    LinkModePreference.preferStatic => StaticLinking(),
    _ => throw UnsupportedError(
      'Unsupported link mode preference: ${codeConfig.linkModePreference}',
    ),
  };
}

String _join(String left, String right) =>
    '$left${Platform.pathSeparator}$right';

void main(List<String> args) async {
  await build(args, (input, output) async {
    if (!input.config.buildCodeAssets) {
      return;
    }

    final ownershipTestHooks = input.userDefines['ownership_test_hooks'];
    if (ownershipTestHooks is! bool?) {
      throw const FormatException(
        'hooks.user_defines.kineto.ownership_test_hooks must be a boolean',
      );
    }

    final appRoot = Directory.fromUri(input.packageRoot).absolute;
    final workspaceRoot = appRoot.parent.parent;
    final rustRoot = Directory(_join(appRoot.path, 'rust'));
    final manifest = File(_join(rustRoot.path, 'Cargo.toml'));
    final appToolchain = File(_join(rustRoot.path, 'rust-toolchain.toml'));
    final rootToolchain = File(
      _join(workspaceRoot.path, 'rust-toolchain.toml'),
    );

    final appChannel = readToolchainChannel(appToolchain);
    final rootChannel = readToolchainChannel(rootToolchain);
    if (appChannel != rootChannel) {
      throw StateError(
        'Rust toolchain pin mismatch: app=$appChannel root=$rootChannel',
      );
    }

    output.dependencies.addAll(nativeBuildInputs(packageRoot: appRoot));

    final codeConfig = input.config.code;
    final targetTriple = targetTripleFor(codeConfig);
    final linkMode = linkModeFor(codeConfig);

    // Toolchain/target provisioning belongs to developer setup and CI. The
    // hook only verifies prerequisites and never mutates the global rustup
    // installation. This keeps host builds cheap and cross/CI targets explicit.
    final toolchainInstalled = await _rustToolchainInstalled(
      channel: appChannel,
      workingDirectory: appRoot.path,
    );
    if (!toolchainInstalled) {
      throw StateError(
        'Rust toolchain $appChannel is not installed for this build. '
        'Install it before building: ${rustToolchainInstallCommand(appChannel)}',
      );
    }

    final targetInstalled = await _rustTargetInstalled(
      channel: appChannel,
      targetTriple: targetTriple,
      workingDirectory: appRoot.path,
    );
    if (!targetInstalled) {
      throw StateError(
        'Rust target $targetTriple is not installed for toolchain $appChannel. '
        'Install it before building: ${rustTargetInstallCommand(channel: appChannel, targetTriple: targetTriple)}',
      );
    }

    // Share Cargo outputs across hook configurations. Cargo already isolates
    // target triples and profiles below one target directory.
    final targetDir = Directory(
      _join(Directory.fromUri(input.outputDirectoryShared).path, 'target'),
    );
    final cargoArgs = <String>[
      'run',
      appChannel,
      'cargo',
      'build',
      '--release',
      '--locked',
      '--manifest-path',
      manifest.path,
      '--package',
      _crateName,
      '--target',
      targetTriple,
      '--target-dir',
      targetDir.path,
      if (ownershipTestHooks == true) ...<String>[
        '--features',
        'ownership-test-hooks',
      ],
    ];
    await _run(
      'rustup',
      cargoArgs,
      workingDirectory: workspaceRoot.path,
      environment: _cargoEnvironment(),
    );

    final libraryFileName = codeConfig.targetOS.libraryFileName(
      _libraryName,
      linkMode,
    );
    final library = File(
      _join(
        _join(
          _join(targetDir.path, targetTriple),
          'release',
        ),
        libraryFileName,
      ),
    );
    if (!library.existsSync()) {
      throw StateError('Cargo succeeded but native library is missing: ${library.path}');
    }

    output.assets.code.add(
      CodeAsset(
        package: input.packageName,
        name: _assetName,
        linkMode: linkMode,
        file: library.absolute.uri,
      ),
      routing: const ToAppBundle(),
    );
  });
}
