import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

part 'kineto_project.dart';

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
const int kinetoExpectedNativeAbiVersion = 9;

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
  reaction(1, 'Reaction'),
  spatialClarity(2, 'Spatial clarity'),
  intimacy(3, 'Intimacy'),
  tension(4, 'Tension');

  const KinetoShotDirection(this.wireValue, this.label);

  factory KinetoShotDirection.fromWire(int value) => switch (value) {
        1 => reaction,
        2 => spatialClarity,
        3 => intimacy,
        4 => tension,
        _ => reaction,
      };

  final int wireValue;
  final String label;
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
  });

  factory KinetoShotSnapshot.fromBits(int bits) {
    final selectedCode = (bits >> 16) & 0xff;
    return KinetoShotSnapshot(
      direction: KinetoShotDirection.fromWire((bits >> 24) & 0xff),
      generated: (bits & 1) != 0,
      locked: ((bits >> 1) & 1) != 0,
      stale: ((bits >> 2) & 1) != 0,
      candidateCount: (bits >> 8) & 0xff,
      selectedIndex: selectedCode == 0 ? null : selectedCode - 1,
      generation: (bits >> 32) & 0xffff,
      supersededCount: (bits >> 48) & 0xffff,
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
