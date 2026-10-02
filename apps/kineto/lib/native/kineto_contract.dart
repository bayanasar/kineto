part of 'kineto_engine.dart';

@Native<Uint32 Function()>(symbol: 'kineto_contract_entry_count', isLeaf: true)
external int _kinetoContractEntryCount();

@Native<
    Int32 Function(
      Uint32,
      Pointer<Uint32>,
      Pointer<Int64>,
      Pointer<Uint32>,
      Pointer<Uint8>,
      Uint64,
      Pointer<Uint64>,
    )>(symbol: 'kineto_contract_entry', isLeaf: true)
external int _kinetoContractEntry(
  int index,
  Pointer<Uint32> outKind,
  Pointer<Int64> outValue,
  Pointer<Uint32> outExtra,
  Pointer<Uint8> outName,
  int outCapacity,
  Pointer<Uint64> outLength,
);

@Native<Uint64 Function()>(symbol: 'kineto_contract_snapshot_sample', isLeaf: true)
external int _kinetoContractSnapshotSample();

/// The native contract, read from the bundled library instead of copied.
///
/// Status codes, shot direction codes and the packed shot-snapshot layout are
/// defined once, in Rust. Snapshots are decoded with the layout read here;
/// the named constants Dart keeps for control flow are checked against these
/// tables by tests.
final class KinetoNativeContract {
  KinetoNativeContract._(
    this.statuses,
    this.directions,
    this._fields,
    this.imageProviders,
    this.snapshotSample,
  );

  /// Loaded once, on first use, after the ABI revision check.
  static final KinetoNativeContract instance = _load();

  /// Project status code by name, for example `busy` → 117.
  final Map<String, int> statuses;

  /// Shot direction wire code by name, for example `tension` → 4.
  final Map<String, int> directions;

  final Map<String, (int, int)> _fields;

  /// Image provider code by name, for example `minimax` → 2.
  final Map<String, int> imageProviders;

  /// A fixed snapshot packed by the native encoder, for contract tests.
  final int snapshotSample;

  /// Names of the packed snapshot's fields.
  Iterable<String> get snapshotFields => _fields.keys;

  /// Read the named field out of packed snapshot [bits].
  int field(int bits, String name) {
    final layout = _fields[name];
    if (layout == null) {
      throw StateError('Kineto native snapshot has no field "$name"');
    }
    final (shift, width) = layout;
    return (bits >>> shift) & ((1 << width) - 1);
  }

  static KinetoNativeContract _load() {
    _assertAbiCompatible();
    const statusKind = 1;
    const directionKind = 2;
    const fieldKind = 3;
    const imageProviderKind = 4;
    final statuses = <String, int>{};
    final imageProviders = <String, int>{};
    final directions = <String, int>{};
    final fields = <String, (int, int)>{};
    final arena = Arena();
    try {
      final kind = arena<Uint32>();
      final value = arena<Int64>();
      final extra = arena<Uint32>();
      final length = arena<Uint64>();
      const capacity = 64;
      final name = arena<Uint8>(capacity);
      for (var index = 0; index < _kinetoContractEntryCount(); index++) {
        _checkProjectResult(
          _kinetoContractEntry(index, kind, value, extra, name, capacity, length),
        );
        final entryName = utf8.decode(name.asTypedList(length.value));
        switch (kind.value) {
          case statusKind:
            statuses[entryName] = value.value;
          case directionKind:
            directions[entryName] = value.value;
          case fieldKind:
            fields[entryName] = (value.value, extra.value);
          case imageProviderKind:
            imageProviders[entryName] = value.value;
        }
      }
    } finally {
      arena.releaseAll();
    }
    return KinetoNativeContract._(
      Map.unmodifiable(statuses),
      Map.unmodifiable(directions),
      Map.unmodifiable(fields),
      Map.unmodifiable(imageProviders),
      _kinetoContractSnapshotSample(),
    );
  }
}
