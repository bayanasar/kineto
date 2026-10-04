import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:isolate';

import 'package:ffi/ffi.dart';

part 'kineto_contract.dart';
part 'kineto_documents.dart';
part 'kineto_handoff.dart';
part 'kineto_planning.dart';
part 'kineto_plugins.dart';
part 'kineto_production.dart';
part 'kineto_project.dart';
part 'kineto_voices.dart';

typedef _DestroyEngineNative = Void Function(Pointer<Void>);

@Native<Pointer<Void> Function()>(symbol: 'kineto_engine_create')
external Pointer<Void> _kinetoEngineCreate();

@Native<_DestroyEngineNative>(symbol: 'kineto_engine_destroy')
external void _kinetoEngineDestroy(Pointer<Void> engine);

@Native<Uint32 Function()>(
  symbol: 'kineto_engine_abi_version',
  isLeaf: true,
)
external int _kinetoEngineAbiVersion();

/// Native ABI revision these Dart bindings were written against.
///
/// The bundled Rust library and these bindings are versioned together. Every
/// fixed-width snapshot layout, status code, and exported symbol belongs to
/// exactly this revision. A mismatched library is refused before decoding.
const int kinetoExpectedNativeAbiVersion = 40;

final class KinetoAbiMismatchException implements Exception {
  const KinetoAbiMismatchException(this.expected, this.actual);

  final int expected;
  final int actual;

  @override
  String toString() =>
      'KinetoAbiMismatchException: bindings expect native ABI $expected, '
      'loaded library reports $actual';
}

void _assertAbiCompatible() {
  final actual = _kinetoEngineAbiVersion();
  if (actual != kinetoExpectedNativeAbiVersion) {
    throw KinetoAbiMismatchException(kinetoExpectedNativeAbiVersion, actual);
  }
}

final class KinetoEngineSnapshot {
  const KinetoEngineSnapshot({required this.abiVersion});

  final int abiVersion;
}

enum KinetoShotDirection {
  reaction(1, 'reaction', 'Reaction'),
  spatialClarity(2, 'spatial_clarity', 'Spatial clarity'),
  intimacy(3, 'intimacy', 'Intimacy'),
  tension(4, 'tension', 'Tension');

  const KinetoShotDirection(this.wireValue, this.wireName, this.label);

  factory KinetoShotDirection.fromWire(int value) => switch (value) {
        1 => reaction,
        2 => spatialClarity,
        3 => intimacy,
        4 => tension,
        _ => reaction,
      };

  final int wireValue;

  /// The name the native contract gives this direction.
  final String wireName;
  final String label;
}

/// Where a shot's video stands.
enum KinetoVideoState {
  none('none'),
  /// Submitted; MiniMax is generating it in the background.
  generating('generating'),
  /// Generated from the locked keyframe and awaiting approval.
  ready('ready'),
  locked('locked'),
  /// Made from a keyframe that is no longer the locked one.
  stale('stale'),
  /// The last submission may have been accepted, and charged, by MiniMax
  /// without leaving its task; generating again waits for the director.
  unresolved('unresolved');

  const KinetoVideoState(this.wireName);

  /// The name the native contract gives this state.
  final String wireName;
}

final class KinetoShotSnapshot {
  const KinetoShotSnapshot({
    required this.direction,
    required this.generated,
    required this.stale,
    required this.selectedIndex,
    required this.locked,
    required this.candidateCount,
    required this.generation,
    required this.supersededCount,
    this.migrationPending = false,
    this.hasInvalidHashes = false,
    this.videoState = KinetoVideoState.none,
  });

  /// Decode a packed native snapshot with the layout the native library
  /// publishes; the bit positions are not repeated here.
  factory KinetoShotSnapshot.fromBits(int bits) {
    final contract = KinetoNativeContract.instance;
    int field(String name) => contract.field(bits, name);
    final selectedCode = field('selected_plus_one');
    return KinetoShotSnapshot(
      direction: KinetoShotDirection.fromWire(field('direction')),
      generated: field('generated') != 0,
      locked: field('locked') != 0,
      stale: field('stale') != 0,
      candidateCount: field('candidate_count'),
      selectedIndex: selectedCode == 0 ? null : selectedCode - 1,
      generation: field('generation_revision'),
      supersededCount: field('superseded_count'),
      migrationPending: field('migration_pending') != 0,
      hasInvalidHashes: field('invalid_hashes') != 0,
      videoState: contract.videoState(field('video_state')),
    );
  }

  final KinetoShotDirection direction;
  final bool generated;
  final bool stale;
  final int? selectedIndex;
  final bool locked;
  final int candidateCount;
  final int generation;
  final int supersededCount;

  /// Stored hashes use a known legacy form. The shot is readable, and changes
  /// are refused until the user migrates it explicitly.
  final bool migrationPending;

  /// Stored hashes this build cannot recognise. The shot stays readable and
  /// every change is refused.
  final bool hasInvalidHashes;

  bool get changesBlocked => migrationPending || hasInvalidHashes;

  final KinetoVideoState videoState;

  /// Native migration refuses while any hash is unrecognised, so it is only
  /// offered for a shot whose sole problem is the known legacy form.
  bool get canMigrate => migrationPending && !hasInvalidHashes;
}

/// Thin owner for the in-process Rust engine.
///
/// Canonical project state is owned by [KinetoProjectSession], not by this
/// process-global handle. No JSON, RPC dispatcher, localhost socket, child
/// engine process, or dynamic library path is involved in the desktop boundary.
final class KinetoEngine implements Finalizable {
  KinetoEngine._(this._handle) {
    _finalizer.attach(this, _handle, detach: this);
  }

  static final NativeFinalizer _finalizer = NativeFinalizer(
    Native.addressOf<NativeFunction<_DestroyEngineNative>>(_kinetoEngineDestroy),
  );

  final Pointer<Void> _handle;
  bool _closed = false;

  factory KinetoEngine.open() {
    _assertAbiCompatible();
    final handle = _kinetoEngineCreate();
    if (handle == nullptr) {
      throw StateError('Failed to initialize the Kineto Rust engine');
    }
    return KinetoEngine._(handle);
  }

  KinetoEngineSnapshot get snapshot {
    _ensureOpen();
    return KinetoEngineSnapshot(abiVersion: _kinetoEngineAbiVersion());
  }

  void close() {
    if (_closed) return;
    _closed = true;
    _finalizer.detach(this);
    _kinetoEngineDestroy(_handle);
  }

  void _ensureOpen() {
    if (_closed) {
      throw StateError('Kineto Rust engine is closed');
    }
  }
}
