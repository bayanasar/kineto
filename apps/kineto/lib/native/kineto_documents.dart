part of 'kineto_engine.dart';

@Native<
  Int32 Function(
    Pointer<Void>,
    Pointer<Uint8>,
    Uint64,
    Pointer<Uint8>,
    Uint64,
    Uint32,
    Uint64,
  )
>(symbol: 'kineto_project_import_source')
external int _kinetoProjectImportSource(
  Pointer<Void> session,
  Pointer<Uint8> text,
  int textLength,
  Pointer<Uint8> name,
  int nameLength,
  int format,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Bool, Uint64)>(
  symbol: 'kineto_project_analyze_story',
)
external int _kinetoProjectAnalyzeStory(Pointer<Void> session, bool resubmit, int token);

@Native<Int32 Function(Pointer<Void>, Bool, Uint64)>(
  symbol: 'kineto_project_write_screenplay',
)
external int _kinetoProjectWriteScreenplay(Pointer<Void> session, bool resubmit, int token);

@Native<
  Int32 Function(
    Pointer<Void>,
    Pointer<Uint8>,
    Uint64,
    Pointer<Uint8>,
    Uint64,
    Bool,
    Uint64,
  )
>(symbol: 'kineto_project_rewrite_scene')
external int _kinetoProjectRewriteScene(
  Pointer<Void> session,
  Pointer<Uint8> sceneId,
  int sceneIdLength,
  Pointer<Uint8> direction,
  int directionLength,
  bool resubmit,
  int token,
);

/// One block of an edited scene, laid out as `KinetoScreenplayBlock`.
final class _NativeScreenplayBlock extends Struct {
  @Uint32()
  external int kind;

  external Pointer<Uint8> character;

  @Uint64()
  external int characterLength;

  external Pointer<Uint8> parenthetical;

  @Uint64()
  external int parentheticalLength;

  external Pointer<Uint8> text;

  @Uint64()
  external int textLength;
}

@Native<
  Int32 Function(
    Pointer<Void>,
    Uint32,
    Pointer<Uint8>,
    Uint64,
    Pointer<Uint8>,
    Uint64,
    Pointer<Uint8>,
    Uint64,
    Pointer<Uint8>,
    Uint64,
    Pointer<_NativeScreenplayBlock>,
    Uint64,
    Uint64,
  )
>(symbol: 'kineto_project_save_scene')
external int _kinetoProjectSaveScene(
  Pointer<Void> session,
  int basedOn,
  Pointer<Uint8> sceneId,
  int sceneIdLength,
  Pointer<Uint8> heading,
  int headingLength,
  Pointer<Uint8> time,
  int timeLength,
  Pointer<Uint8> goal,
  int goalLength,
  Pointer<_NativeScreenplayBlock> blocks,
  int blockCount,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint64)>(
  symbol: 'kineto_project_lock_screenplay',
)
external int _kinetoProjectLockScreenplay(
  Pointer<Void> session,
  int revision,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Pointer<Uint64>)>(
  symbol: 'kineto_project_documents_state',
)
external int _kinetoProjectDocumentsState(
  Pointer<Void> session,
  Pointer<Uint64> out,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Pointer<Uint32>)>(
  symbol: 'kineto_project_screenplay_count',
)
external int _kinetoProjectScreenplayCount(
  Pointer<Void> session,
  int what,
  int scene,
  Pointer<Uint32> out,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Uint32, Pointer<Uint64>)>(
  symbol: 'kineto_project_screenplay_value',
)
external int _kinetoProjectScreenplayValue(
  Pointer<Void> session,
  int field,
  int scene,
  int item,
  Pointer<Uint64> out,
);

@Native<
  Int32 Function(
    Pointer<Void>,
    Uint32,
    Uint32,
    Uint32,
    Pointer<Uint8>,
    Uint64,
    Pointer<Uint64>,
  )
>(symbol: 'kineto_project_screenplay_text_copy')
external int _kinetoProjectScreenplayTextCopy(
  Pointer<Void> session,
  int field,
  int scene,
  int item,
  Pointer<Uint8> outBuffer,
  int outCapacity,
  Pointer<Uint64> outLength,
);

@Native<Uint32 Function(Pointer<Void>)>(
  symbol: 'kineto_project_llm_provider',
  isLeaf: true,
)
external int _kinetoProjectLlmProvider(Pointer<Void> session);

@Native<Uint8 Function(Pointer<Void>)>(
  symbol: 'kineto_project_llm_ready',
  isLeaf: true,
)
external int _kinetoProjectLlmReady(Pointer<Void> session);

@Native<Int32 Function(Pointer<Void>, Pointer<Uint8>, Uint64, Pointer<Uint64>)>(
  symbol: 'kineto_project_llm_model_copy',
)
external int _kinetoProjectLlmModelCopy(
  Pointer<Void> session,
  Pointer<Uint8> outBuffer,
  int outCapacity,
  Pointer<Uint64> outLength,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Pointer<Uint64>)>(
  symbol: 'kineto_project_document_cost',
)
external int _kinetoProjectDocumentCost(
  Pointer<Void> session,
  int document,
  Pointer<Uint64> out,
);

/// A document the engine writes with a language model.
enum KinetoDocumentKind {
  storyAnalysis('analysis', 'Story analysis'),
  screenplay('screenplay', 'Screenplay'),
  bible('bible', 'Bible');

  const KinetoDocumentKind(this.wireName, this.label);

  /// The name the native contract's document costs use.
  final String wireName;
  final String label;
}

/// Which language model a session writes documents with.
enum KinetoLanguageModelKind {
  none('none', 'No model'),
  kimiCode('kimi_code', 'Kimi Code'),
  kimi('kimi', 'Kimi API'),
  local('local', 'Local model');

  const KinetoLanguageModelKind(this.wireName, this.label);

  final String wireName;
  final String label;
}

/// The session's language model, as the director needs to know it before
/// asking for a document.
final class KinetoLanguageModel {
  const KinetoLanguageModel({
    required this.kind,
    required this.ready,
    this.model = '',
  });

  static const KinetoLanguageModel none = KinetoLanguageModel(
    kind: KinetoLanguageModelKind.none,
    ready: false,
  );

  final KinetoLanguageModelKind kind;

  /// False when nothing can be asked: no model named, or one without its key
  /// or command-line tool.
  final bool ready;

  /// The model, as far as Kineto knows it; an agent uses its own default.
  final String model;

  /// An agent signs in on its own and is billed by its subscription.
  bool get isAgent => kind == KinetoLanguageModelKind.kimiCode;

  String get label =>
      model.isEmpty || isAgent ? kind.label : '${kind.label} · $model';
}

enum KinetoSourceFormat {
  text('text'),
  markdown('markdown');

  const KinetoSourceFormat(this.wireName);

  final String wireName;
}

enum KinetoBlockKind {
  action('action', 'Action'),
  dialogue('dialogue', 'Dialogue'),
  transition('transition', 'Transition');

  const KinetoBlockKind(this.wireName, this.label);

  final String wireName;
  final String label;
}

enum KinetoSceneSetting {
  interior('interior', 'Interior'),
  exterior('exterior', 'Exterior'),
  interiorExterior('interior_exterior', 'Interior / exterior');

  const KinetoSceneSetting(this.wireName, this.label);

  final String wireName;
  final String label;
}

enum KinetoRevisionStatus {
  candidate('candidate'),
  locked('locked'),
  superseded('superseded');

  const KinetoRevisionStatus(this.wireName);

  final String wireName;
}

enum KinetoRevisionOrigin {
  generated('generated'),
  edited('edited');

  const KinetoRevisionOrigin(this.wireName);

  final String wireName;
}

T _byWireName<T extends Enum>(
  List<T> values,
  String Function(T) wireName,
  String name,
) {
  for (final value in values) {
    if (wireName(value) == name) return value;
  }
  throw StateError('Kineto native contract names unknown value "$name"');
}

final class KinetoScreenplayBlock {
  const KinetoScreenplayBlock({
    required this.kind,
    required this.text,
    this.character,
    this.parenthetical,
  });

  final KinetoBlockKind kind;

  /// The cast ID of who speaks; dialogue only.
  final String? character;

  /// A short direction for the line, without parentheses; dialogue only.
  final String? parenthetical;
  final String text;

  @override
  bool operator ==(Object other) =>
      other is KinetoScreenplayBlock &&
      other.kind == kind &&
      other.character == character &&
      other.parenthetical == parenthetical &&
      other.text == text;

  @override
  int get hashCode => Object.hash(kind, character, parenthetical, text);
}

final class KinetoCastMember {
  const KinetoCastMember({required this.id, required this.name});

  final String id;
  final String name;
}

final class KinetoScene {
  const KinetoScene({
    required this.id,
    required this.heading,
    required this.setting,
    required this.location,
    required this.time,
    required this.characters,
    required this.dramaticGoal,
    required this.blocks,
  });

  final String id;
  final String heading;
  final KinetoSceneSetting setting;

  /// The story analysis location the scene is set in.
  final String location;
  final String time;

  /// Cast IDs of everyone present or heard.
  final List<String> characters;
  final String dramaticGoal;
  final List<KinetoScreenplayBlock> blocks;
}

final class KinetoScreenplay {
  const KinetoScreenplay({
    required this.revision,
    required this.title,
    required this.cast,
    required this.scenes,
  });

  /// The revision this is: the latest one still in play.
  final int revision;
  final String title;
  final List<KinetoCastMember> cast;
  final List<KinetoScene> scenes;

  /// The name dialogue by [castId] is attributed to.
  String nameOf(String castId) {
    for (final member in cast) {
      if (member.id == castId) return member.name;
    }
    return castId;
  }
}

final class KinetoRevision {
  const KinetoRevision({
    required this.number,
    required this.status,
    required this.origin,
    this.basedOn,
    this.provider = '',
    this.model = '',
  });

  final int number;
  final KinetoRevisionStatus status;
  final KinetoRevisionOrigin origin;

  /// The revision an edit or a rewritten scene started from.
  final int? basedOn;

  /// Who wrote a generated revision; empty for the director's edits.
  final String provider;
  final String model;
}

final class KinetoStoryAnalysis {
  const KinetoStoryAnalysis({
    required this.revision,
    required this.logline,
    required this.characterNames,
  });

  final int revision;
  final String logline;
  final List<String> characterNames;
}

/// The project's documents as the engine last published them.
final class KinetoDocuments {
  const KinetoDocuments({
    this.sourceRevision,
    this.analysis,
    this.screenplay,
    this.revisions = const <KinetoRevision>[],
    this.lockedRevision,
    this.analysisStale = false,
    this.screenplayStale = false,
    this.damaged = false,
  });

  final int? sourceRevision;
  final KinetoStoryAnalysis? analysis;

  /// The latest screenplay revision still in play: the one the director
  /// works on.
  final KinetoScreenplay? screenplay;

  /// Every screenplay revision, oldest first.
  final List<KinetoRevision> revisions;

  /// The revision production follows.
  final int? lockedRevision;

  /// The analysis read a source revision that is no longer current.
  final bool analysisStale;

  /// The screenplay was written from an analysis that is no longer current.
  final bool screenplayStale;

  /// The documents could not be read.
  final bool damaged;

  /// The screenplay the director works on is the one production follows.
  bool get screenplayLocked =>
      screenplay != null && lockedRevision == screenplay!.revision;
}

/// Document commands and reads. Every command queues on the project's
/// worker and completes when it has run; reads return published state and
/// never do I/O.
extension KinetoProjectDocuments on KinetoProjectSession {
  /// Import [text] as the next source revision.
  Future<void> importSource(
    String text, {
    KinetoSourceFormat format = KinetoSourceFormat.text,
    String? originalName,
  }) {
    final contract = KinetoNativeContract.instance;
    return _submitDocument((token) {
      final arena = Arena();
      try {
        final textBytes = _encodeUtf8(arena, text);
        final nameBytes = _encodeUtf8(arena, originalName ?? '');
        return _kinetoProjectImportSource(
          _handle,
          textBytes.pointer,
          textBytes.length,
          nameBytes.pointer,
          nameBytes.length,
          contract.code(contract.sourceFormats, format.wireName),
          token,
        );
      } finally {
        arena.releaseAll();
      }
    });
  }

  /// Analyse the current source with the session's language model.
  ///
  /// When the last paid request for the analysis may have been answered,
  /// and charged for, without its answer being kept, this fails with
  /// [KinetoProjectException.paidCallUnresolved] unless [resubmit] is the
  /// director's decision to risk paying twice. The same holds for
  /// [writeScreenplay] and [rewriteScene].
  Future<void> analyzeStory({bool resubmit = false}) =>
      _submitDocument((token) => _kinetoProjectAnalyzeStory(_handle, resubmit, token));

  /// Write a screenplay of the current story analysis.
  Future<void> writeScreenplay({bool resubmit = false}) =>
      _submitDocument((token) => _kinetoProjectWriteScreenplay(_handle, resubmit, token));

  /// Write scene [sceneId] of the latest screenplay again, as [direction]
  /// asks.
  Future<void> rewriteScene(String sceneId, {String? direction, bool resubmit = false}) =>
      _submitDocument((token) {
        final arena = Arena();
        try {
          final idBytes = _encodeUtf8(arena, sceneId);
          final directionBytes = _encodeUtf8(arena, direction ?? '');
          return _kinetoProjectRewriteScene(
            _handle,
            idBytes.pointer,
            idBytes.length,
            directionBytes.pointer,
            directionBytes.length,
            resubmit,
            token,
          );
        } finally {
          arena.releaseAll();
        }
      });

  /// Save the director's edit of scene [sceneId] of revision [basedOn],
  /// which must be the latest. Fails with [KinetoProjectException.notLatest]
  /// when a newer revision exists.
  Future<void> saveScene({
    required int basedOn,
    required String sceneId,
    required String heading,
    required String time,
    required String dramaticGoal,
    required List<KinetoScreenplayBlock> blocks,
  }) {
    final contract = KinetoNativeContract.instance;
    return _submitDocument((token) {
      final arena = Arena();
      try {
        final id = _encodeUtf8(arena, sceneId);
        final headingBytes = _encodeUtf8(arena, heading);
        final timeBytes = _encodeUtf8(arena, time);
        final goal = _encodeUtf8(arena, dramaticGoal);
        final native = arena<_NativeScreenplayBlock>(
          blocks.isEmpty ? 1 : blocks.length,
        );
        for (var index = 0; index < blocks.length; index++) {
          final block = blocks[index];
          final character = _encodeUtf8(arena, block.character ?? '');
          final parenthetical = _encodeUtf8(arena, block.parenthetical ?? '');
          final text = _encodeUtf8(arena, block.text);
          native[index]
            ..kind = contract.code(contract.blockKinds, block.kind.wireName)
            ..character = character.pointer
            ..characterLength = character.length
            ..parenthetical = parenthetical.pointer
            ..parentheticalLength = parenthetical.length
            ..text = text.pointer
            ..textLength = text.length;
        }
        return _kinetoProjectSaveScene(
          _handle,
          basedOn,
          id.pointer,
          id.length,
          headingBytes.pointer,
          headingBytes.length,
          timeBytes.pointer,
          timeBytes.length,
          goal.pointer,
          goal.length,
          native,
          blocks.length,
          token,
        );
      } finally {
        arena.releaseAll();
      }
    });
  }

  /// Lock screenplay [revision] as the one production follows.
  Future<void> lockScreenplay(int revision) => _submitDocument(
    (token) => _kinetoProjectLockScreenplay(_handle, revision, token),
  );

  /// The model this session writes documents with. Fixed when it opened.
  KinetoLanguageModel get languageModel {
    _ensureOpen();
    final contract = KinetoNativeContract.instance;
    return KinetoLanguageModel(
      kind: _byWireName(
        KinetoLanguageModelKind.values,
        (kind) => kind.wireName,
        contract.nameOf(
          contract.llmProviders,
          _kinetoProjectLlmProvider(_handle),
        ),
      ),
      ready: _kinetoProjectLlmReady(_handle) != 0,
      model: _copyProjectUtf8(
        _handle,
        _kinetoProjectLlmModelCopy,
        'language model',
      ),
    );
  }

  /// Estimated cost of asking for [document], in USD micros; null when it
  /// is not known (an agent on a subscription) or cannot be asked for yet.
  int? documentCostMicros(KinetoDocumentKind document) {
    _ensureOpen();
    final contract = KinetoNativeContract.instance;
    final arena = Arena();
    try {
      final out = arena<Uint64>();
      final code = _kinetoProjectDocumentCost(
        _handle,
        contract.code(contract.documentCosts, document.wireName),
        out,
      );
      if (code == 0) return out.value;
      if (code == KinetoProjectException.costUnknown ||
          code == KinetoProjectException.modelNotConfigured ||
          code == KinetoProjectException.needsAnalysis ||
          code == KinetoProjectException.needsLockedScreenplay) {
        return null;
      }
      throw _projectException(code);
    } finally {
      arena.releaseAll();
    }
  }

  /// The project's documents as last published.
  KinetoDocuments get documents {
    _ensureOpen();
    final reader = _DocumentReader(_handle);
    try {
      return reader.read();
    } finally {
      reader.release();
    }
  }
}

/// Reads the published documents field by field.
final class _DocumentReader {
  _DocumentReader(this._handle);

  final Pointer<Void> _handle;
  final Arena _arena = Arena();
  final KinetoNativeContract _contract = KinetoNativeContract.instance;
  late final Pointer<Uint32> _count = _arena<Uint32>();
  late final Pointer<Uint64> _value = _arena<Uint64>();

  void release() => _arena.releaseAll();

  int count(String what, [int scene = 0]) {
    _checkProjectResult(
      _kinetoProjectScreenplayCount(
        _handle,
        _contract.code(_contract.screenplayCounts, what),
        scene,
        _count,
      ),
    );
    return _count.value;
  }

  int value(String field, {int scene = 0, int item = 0}) {
    _checkProjectResult(
      _kinetoProjectScreenplayValue(
        _handle,
        _contract.code(_contract.screenplayValues, field),
        scene,
        item,
        _value,
      ),
    );
    return _value.value;
  }

  String text(String field, {int scene = 0, int item = 0}) {
    final code = _contract.code(_contract.screenplayTexts, field);
    return _copyProjectUtf8(
      _handle,
      (session, buffer, capacity, length) => _kinetoProjectScreenplayTextCopy(
        session,
        code,
        scene,
        item,
        buffer,
        capacity,
        length,
      ),
      field,
    );
  }

  T named<T extends Enum>(
    List<T> values,
    String Function(T) wireName,
    Map<String, int> table,
    int code,
  ) => _byWireName(values, wireName, _contract.nameOf(table, code));

  KinetoDocuments read() {
    _checkProjectResult(_kinetoProjectDocumentsState(_handle, _value));
    final bits = _value.value;
    int? revision(String field) {
      final value = _contract.documentField(bits, field);
      return value == 0 ? null : value;
    }

    final analysisRevision = revision('analysis_revision');
    final screenplayRevision = revision('screenplay_revision');
    return KinetoDocuments(
      sourceRevision: revision('source_revision'),
      analysis: analysisRevision == null
          ? null
          : KinetoStoryAnalysis(
              revision: analysisRevision,
              logline: text('analysis_logline'),
              characterNames: <String>[
                for (var item = 0; item < count('analysis_characters'); item++)
                  text('analysis_character_name', item: item),
              ],
            ),
      screenplay: screenplayRevision == null
          ? null
          : _screenplay(screenplayRevision),
      revisions: <KinetoRevision>[
        for (var item = 0; item < count('revisions'); item++) _revision(item),
      ],
      lockedRevision: revision('locked_revision'),
      analysisStale: _contract.documentField(bits, 'analysis_stale') != 0,
      screenplayStale: _contract.documentField(bits, 'screenplay_stale') != 0,
      damaged: _contract.documentField(bits, 'documents_damaged') != 0,
    );
  }

  KinetoScreenplay _screenplay(int revision) => KinetoScreenplay(
    revision: revision,
    title: text('title'),
    cast: <KinetoCastMember>[
      for (var item = 0; item < count('cast'); item++)
        KinetoCastMember(
          id: text('cast_id', item: item),
          name: text('cast_name', item: item),
        ),
    ],
    scenes: <KinetoScene>[
      for (var scene = 0; scene < count('scenes'); scene++) _scene(scene),
    ],
  );

  KinetoScene _scene(int scene) => KinetoScene(
    id: text('scene_id', scene: scene),
    heading: text('scene_heading', scene: scene),
    setting: named(
      KinetoSceneSetting.values,
      (setting) => setting.wireName,
      _contract.settings,
      value('scene_setting', scene: scene),
    ),
    location: text('scene_location', scene: scene),
    time: text('scene_time', scene: scene),
    characters: <String>[
      for (var item = 0; item < count('scene_characters', scene); item++)
        text('scene_character', scene: scene, item: item),
    ],
    dramaticGoal: text('scene_goal', scene: scene),
    blocks: <KinetoScreenplayBlock>[
      for (var item = 0; item < count('scene_blocks', scene); item++)
        _block(scene, item),
    ],
  );

  KinetoScreenplayBlock _block(int scene, int item) {
    String? optional(String field) {
      final value = text(field, scene: scene, item: item);
      return value.isEmpty ? null : value;
    }

    return KinetoScreenplayBlock(
      kind: named(
        KinetoBlockKind.values,
        (kind) => kind.wireName,
        _contract.blockKinds,
        value('block_kind', scene: scene, item: item),
      ),
      character: optional('block_character'),
      parenthetical: optional('block_parenthetical'),
      text: text('block_text', scene: scene, item: item),
    );
  }

  KinetoRevision _revision(int item) {
    final basedOn = value('revision_based_on', item: item);
    return KinetoRevision(
      number: value('revision_number', item: item),
      status: named(
        KinetoRevisionStatus.values,
        (status) => status.wireName,
        _contract.revisionStatuses,
        value('revision_status', item: item),
      ),
      origin: named(
        KinetoRevisionOrigin.values,
        (origin) => origin.wireName,
        _contract.origins,
        value('revision_origin', item: item),
      ),
      basedOn: basedOn == 0 ? null : basedOn,
      provider: text('revision_provider', item: item),
      model: text('revision_model', item: item),
    );
  }
}
