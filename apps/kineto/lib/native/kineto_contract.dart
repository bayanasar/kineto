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
    this.videoStates,
    this.snapshotSample,
    this._codes,
    this._documentFields,
    this._productionFields,
    this._planFields,
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

  /// Snapshot video state code by name, for example `ready` → 2.
  final Map<String, int> videoStates;

  /// The video state a snapshot's `video_state` code names.
  KinetoVideoState videoState(int code) {
    for (final state in KinetoVideoState.values) {
      if (videoStates[state.wireName] == code) return state;
    }
    throw StateError('Kineto native snapshot has unknown video state $code');
  }

  /// A fixed snapshot packed by the native encoder, for contract tests.
  final int snapshotSample;

  final Map<int, Map<String, int>> _codes;
  final Map<String, (int, int)> _documentFields;
  final Map<String, (int, int)> _productionFields;
  final Map<String, (int, int)> _planFields;

  static const int _llmProviderKind = 6;
  static const int _documentFieldKind = 7;
  static const int _screenplayCountKind = 8;
  static const int _screenplayValueKind = 9;
  static const int _screenplayTextKind = 10;
  static const int _blockKindKind = 11;
  static const int _settingKind = 12;
  static const int _revisionStatusKind = 13;
  static const int _originKind = 14;
  static const int _sourceFormatKind = 15;
  static const int _documentCostKind = 16;
  static const int _productionFieldKind = 17;
  static const int _productionCountKind = 18;
  static const int _productionValueKind = 19;
  static const int _productionTextKind = 20;
  static const int _staleCauseKind = 21;
  static const int _choiceStatusKind = 22;
  static const int _castImageKindKind = 23;
  static const int _entryKindKind = 24;
  static const int _decisionKind = 25;
  static const int _imageCostKind = 26;
  static const int _planFieldKind = 27;
  static const int _planCountKind = 28;
  static const int _planValueKind = 29;
  static const int _planTextKind = 30;
  static const int _framingKind = 31;
  static const int _cameraMotionKind = 32;
  static const int _planStaleCauseKind = 33;
  static const int _stepStateKind = 34;
  static const int _takeCountKind = 35;
  static const int _takeValueKind = 36;
  static const int _takeTextKind = 37;
  static const int _takeStatusKind = 38;
  static const int _videoProviderKind = 39;

  Map<String, int> _table(int kind) => _codes[kind] ?? const <String, int>{};

  /// Language model code by name, for example `kimi_code` → 1.
  Map<String, int> get llmProviders => _table(_llmProviderKind);

  /// What `kineto_project_screenplay_count` counts, by name.
  Map<String, int> get screenplayCounts => _table(_screenplayCountKind);

  /// Number fields of the published documents, by name.
  Map<String, int> get screenplayValues => _table(_screenplayValueKind);

  /// Text fields of the published documents, by name.
  Map<String, int> get screenplayTexts => _table(_screenplayTextKind);

  Map<String, int> get blockKinds => _table(_blockKindKind);
  Map<String, int> get settings => _table(_settingKind);
  Map<String, int> get revisionStatuses => _table(_revisionStatusKind);
  Map<String, int> get origins => _table(_originKind);
  Map<String, int> get sourceFormats => _table(_sourceFormatKind);
  Map<String, int> get documentCosts => _table(_documentCostKind);

  /// What `kineto_project_production_count` counts, by name.
  Map<String, int> get productionCounts => _table(_productionCountKind);

  /// Number fields of the published bible, style and cast, by name.
  Map<String, int> get productionValues => _table(_productionValueKind);

  /// Text fields of the published bible, style and cast, by name.
  Map<String, int> get productionTexts => _table(_productionTextKind);

  Map<String, int> get staleCauses => _table(_staleCauseKind);
  Map<String, int> get choiceStatuses => _table(_choiceStatusKind);
  Map<String, int> get castImageKinds => _table(_castImageKindKind);
  Map<String, int> get entryKinds => _table(_entryKindKind);
  Map<String, int> get decisions => _table(_decisionKind);
  Map<String, int> get imageCosts => _table(_imageCostKind);

  /// What `kineto_project_plan_count` counts, by name.
  Map<String, int> get planCounts => _table(_planCountKind);

  /// Number fields of the published plan and recipe, by name.
  Map<String, int> get planValues => _table(_planValueKind);

  /// Text fields of the published plan and recipe, by name.
  Map<String, int> get planTexts => _table(_planTextKind);

  Map<String, int> get framings => _table(_framingKind);
  Map<String, int> get cameraMotions => _table(_cameraMotionKind);
  Map<String, int> get planStaleCauses => _table(_planStaleCauseKind);
  Map<String, int> get stepStates => _table(_stepStateKind);
  Map<String, int> get takeCounts => _table(_takeCountKind);
  Map<String, int> get takeValues => _table(_takeValueKind);
  Map<String, int> get takeTexts => _table(_takeTextKind);
  Map<String, int> get takeStatuses => _table(_takeStatusKind);
  Map<String, int> get videoProviders => _table(_videoProviderKind);

  /// Names of the packed plan state's fields.
  Iterable<String> get planFields => _planFields.keys;

  /// Read the named field out of a packed plan state.
  int planField(int bits, String name) {
    final layout = _planFields[name];
    if (layout == null) {
      throw StateError('Kineto native plan state has no field "$name"');
    }
    final (shift, width) = layout;
    return (bits >>> shift) & ((1 << width) - 1);
  }

  /// Names of the packed production state's fields.
  Iterable<String> get productionFields => _productionFields.keys;

  /// Read the named field out of a packed production state.
  int productionField(int bits, String name) {
    final layout = _productionFields[name];
    if (layout == null) {
      throw StateError('Kineto native production state has no field "$name"');
    }
    final (shift, width) = layout;
    return (bits >>> shift) & ((1 << width) - 1);
  }

  /// Names of the packed documents state's fields.
  Iterable<String> get documentFields => _documentFields.keys;

  /// The code [table] gives [name]; a name the library lacks is a contract
  /// break, not a runtime condition.
  int code(Map<String, int> table, String name) {
    final code = table[name];
    if (code == null) {
      throw StateError('Kineto native contract has no code for "$name"');
    }
    return code;
  }

  /// The name [table] gives [code].
  String nameOf(Map<String, int> table, int code) {
    for (final entry in table.entries) {
      if (entry.value == code) return entry.key;
    }
    throw StateError('Kineto native contract has no name for code $code');
  }

  /// Read the named field out of a packed documents state.
  int documentField(int bits, String name) {
    final layout = _documentFields[name];
    if (layout == null) {
      throw StateError('Kineto native documents state has no field "$name"');
    }
    final (shift, width) = layout;
    return (bits >>> shift) & ((1 << width) - 1);
  }

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
    const videoStateKind = 5;
    final statuses = <String, int>{};
    final imageProviders = <String, int>{};
    final videoStates = <String, int>{};
    final directions = <String, int>{};
    final fields = <String, (int, int)>{};
    final codes = <int, Map<String, int>>{};
    final documentFields = <String, (int, int)>{};
    final productionFields = <String, (int, int)>{};
    final planFields = <String, (int, int)>{};
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
          case videoStateKind:
            videoStates[entryName] = value.value;
          case _documentFieldKind:
            documentFields[entryName] = (value.value, extra.value);
          case _productionFieldKind:
            productionFields[entryName] = (value.value, extra.value);
          case _planFieldKind:
            planFields[entryName] = (value.value, extra.value);
          default:
            (codes[kind.value] ??= <String, int>{})[entryName] = value.value;
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
      Map.unmodifiable(videoStates),
      _kinetoContractSnapshotSample(),
      Map.unmodifiable(<int, Map<String, int>>{
        for (final entry in codes.entries) entry.key: Map.unmodifiable(entry.value),
      }),
      Map.unmodifiable(documentFields),
      Map.unmodifiable(productionFields),
      Map.unmodifiable(planFields),
    );
  }
}
