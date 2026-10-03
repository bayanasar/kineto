part of 'kineto_engine.dart';

@Native<Int32 Function(Pointer<Void>, Bool, Uint64)>(symbol: 'kineto_project_write_bible')
external int _kinetoProjectWriteBible(Pointer<Void> session, bool resubmit, int token);

/// One UTF-8 string, laid out as `KinetoText`.
final class _NativeText extends Struct {
  external Pointer<Uint8> pointer;

  @Uint64()
  external int length;
}

@Native<
  Int32 Function(
    Pointer<Void>,
    Uint32,
    Uint32,
    Pointer<Uint8>,
    Uint64,
    Pointer<Uint8>,
    Uint64,
    Pointer<Uint8>,
    Uint64,
    Pointer<Uint8>,
    Uint64,
    Pointer<_NativeText>,
    Uint64,
    Uint64,
  )
>(symbol: 'kineto_project_save_bible_entry')
external int _kinetoProjectSaveBibleEntry(
  Pointer<Void> session,
  int basedOn,
  int kind,
  Pointer<Uint8> id,
  int idLength,
  Pointer<Uint8> description,
  int descriptionLength,
  Pointer<Uint8> appearance,
  int appearanceLength,
  Pointer<Uint8> wardrobe,
  int wardrobeLength,
  Pointer<_NativeText> forbidden,
  int forbiddenCount,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint64)>(symbol: 'kineto_project_lock_bible')
external int _kinetoProjectLockBible(Pointer<Void> session, int revision, int token);

@Native<Int32 Function(Pointer<Void>, Pointer<Uint8>, Uint64, Pointer<Uint8>, Uint64, Bool, Uint64)>(
  symbol: 'kineto_project_sample_style',
)
external int _kinetoProjectSampleStyle(
  Pointer<Void> session,
  Pointer<Uint8> preset,
  int presetLength,
  Pointer<Uint8> note,
  int noteLength,
  bool resubmit,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Pointer<Uint8>, Uint64, Pointer<Uint8>, Uint64, Bool, Uint64)>(
  symbol: 'kineto_project_refine_style',
)
external int _kinetoProjectRefineStyle(
  Pointer<Void> session,
  Pointer<Uint8> sample,
  int sampleLength,
  Pointer<Uint8> note,
  int noteLength,
  bool resubmit,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Pointer<Uint8>, Uint64, Uint64)>(
  symbol: 'kineto_project_decide_style',
)
external int _kinetoProjectDecideStyle(
  Pointer<Void> session,
  int decision,
  Pointer<Uint8> sample,
  int sampleLength,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Pointer<Uint8>, Uint64, Uint32, Bool, Uint64)>(
  symbol: 'kineto_project_audition',
)
external int _kinetoProjectAudition(
  Pointer<Void> session,
  Pointer<Uint8> character,
  int characterLength,
  int count,
  bool resubmit,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Pointer<Uint8>, Uint64, Pointer<Uint32>, Uint64, Bool, Uint64)>(
  symbol: 'kineto_project_make_pack',
)
external int _kinetoProjectMakePack(
  Pointer<Void> session,
  Pointer<Uint8> character,
  int characterLength,
  Pointer<Uint32> remake,
  int remakeCount,
  bool resubmit,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Pointer<Uint8>, Uint64, Pointer<Uint8>, Uint64, Uint64)>(
  symbol: 'kineto_project_decide_portrait',
)
external int _kinetoProjectDecidePortrait(
  Pointer<Void> session,
  int decision,
  Pointer<Uint8> character,
  int characterLength,
  Pointer<Uint8> portrait,
  int portraitLength,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Pointer<Uint8>, Uint64, Uint64)>(
  symbol: 'kineto_project_lock_character',
)
external int _kinetoProjectLockCharacter(
  Pointer<Void> session,
  Pointer<Uint8> character,
  int characterLength,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Pointer<Uint64>)>(symbol: 'kineto_project_production_state')
external int _kinetoProjectProductionState(Pointer<Void> session, Pointer<Uint64> out);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Pointer<Uint32>)>(
  symbol: 'kineto_project_production_count',
)
external int _kinetoProjectProductionCount(
  Pointer<Void> session,
  int what,
  int group,
  Pointer<Uint32> out,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Uint32, Pointer<Uint64>)>(
  symbol: 'kineto_project_production_value',
)
external int _kinetoProjectProductionValue(
  Pointer<Void> session,
  int field,
  int group,
  int item,
  Pointer<Uint64> out,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Uint32, Pointer<Uint8>, Uint64, Pointer<Uint64>)>(
  symbol: 'kineto_project_production_text_copy',
)
external int _kinetoProjectProductionTextCopy(
  Pointer<Void> session,
  int field,
  int group,
  int item,
  Pointer<Uint8> outBuffer,
  int outCapacity,
  Pointer<Uint64> outLength,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Pointer<Uint64>, Pointer<Uint32>)>(
  symbol: 'kineto_project_image_cost',
)
external int _kinetoProjectImageCost(
  Pointer<Void> session,
  int what,
  Pointer<Uint64> outMicros,
  Pointer<Uint32> outImages,
);

/// Which list of the bible an entry is in.
enum KinetoBibleEntryKind {
  character('character'),
  location('location'),
  prop('prop');

  const KinetoBibleEntryKind(this.wireName);

  final String wireName;
}

/// What a character wears in some scenes instead of their usual wardrobe.
final class KinetoWardrobeChange {
  const KinetoWardrobeChange({required this.scenes, required this.wardrobe});

  /// The scene IDs, comma-separated.
  final String scenes;
  final String wardrobe;
}

/// One entry of the bible: a character, a location or a prop.
final class KinetoBibleEntry {
  const KinetoBibleEntry({
    required this.kind,
    required this.id,
    required this.name,
    required this.description,
    required this.appearance,
    this.wardrobe = '',
    this.wardrobeChanges = const <KinetoWardrobeChange>[],
    this.forbiddenChanges = const <String>[],
    this.continuityNotes = const <String>[],
  });

  final KinetoBibleEntryKind kind;
  final String id;
  final String name;
  final String description;
  final String appearance;

  /// A character's usual wardrobe; empty for other entries.
  final String wardrobe;
  final List<KinetoWardrobeChange> wardrobeChanges;

  /// What must never change from shot to shot.
  final List<String> forbiddenChanges;
  final List<String> continuityNotes;
}

/// The world and character bible every image follows.
final class KinetoBible {
  const KinetoBible({
    required this.revision,
    required this.setting,
    required this.period,
    this.visualRules = const <String>[],
    this.continuityRules = const <String>[],
    this.characters = const <KinetoBibleEntry>[],
    this.locations = const <KinetoBibleEntry>[],
    this.props = const <KinetoBibleEntry>[],
  });

  /// The revision this is: the latest one still in play.
  final int revision;
  final String setting;
  final String period;
  final List<String> visualRules;
  final List<String> continuityRules;
  final List<KinetoBibleEntry> characters;
  final List<KinetoBibleEntry> locations;
  final List<KinetoBibleEntry> props;
}

/// The director's edit of one bible entry.
final class KinetoBibleEntryEdit {
  const KinetoBibleEntryEdit({
    required this.kind,
    required this.id,
    required this.description,
    required this.appearance,
    this.wardrobe = '',
    this.forbiddenChanges = const <String>[],
  });

  final KinetoBibleEntryKind kind;
  final String id;
  final String description;
  final String appearance;

  /// A character's usual wardrobe; ignored for other entries.
  final String wardrobe;
  final List<String> forbiddenChanges;
}

/// Why the bible may no longer describe the locked screenplay.
enum KinetoBibleStaleCause {
  noLockedScreenplay('no_locked_screenplay'),
  material('material'),
  sceneChanged('scene_changed'),
  sceneRemoved('scene_removed'),
  sceneAdded('scene_added');

  const KinetoBibleStaleCause(this.wireName);

  final String wireName;
}

final class KinetoBibleStaleness {
  const KinetoBibleStaleness(this.cause, [this.subject = '']);

  final KinetoBibleStaleCause cause;

  /// The scene or record that changed; empty when none is named.
  final String subject;

  String get description => switch (cause) {
        KinetoBibleStaleCause.noLockedScreenplay => 'no screenplay is locked',
        KinetoBibleStaleCause.material => '$subject changed',
        KinetoBibleStaleCause.sceneChanged => 'scene $subject changed',
        KinetoBibleStaleCause.sceneRemoved => 'scene $subject was removed',
        KinetoBibleStaleCause.sceneAdded => 'scene $subject is new',
      };
}

/// Where the director stands on a style sample or a portrait.
enum KinetoChoiceStatus {
  candidate('candidate'),
  selected('selected'),
  rejected('rejected');

  const KinetoChoiceStatus(this.wireName);

  final String wireName;
}

/// A style direction the director samples from.
final class KinetoStylePreset {
  const KinetoStylePreset({required this.id, required this.name});

  final String id;
  final String name;
}

final class KinetoStyleSample {
  const KinetoStyleSample({
    required this.id,
    required this.preset,
    required this.sceneId,
    required this.status,
    required this.mediaPath,
    this.note,
    this.refines,
    this.provider = '',
    this.model = '',
  });

  final String id;
  final String preset;
  final String sceneId;
  final KinetoChoiceStatus status;

  /// Absolute path of the sample image.
  final String mediaPath;
  final String? note;

  /// The sample this one refines, if any.
  final String? refines;
  final String provider;
  final String model;
}

/// The style every image follows.
final class KinetoStyleLock {
  const KinetoStyleLock({required this.sampleId, required this.preset, this.note});

  final String sampleId;
  final String preset;
  final String? note;
}

enum KinetoCastImageKind {
  portrait('portrait', 'Portrait'),
  front('front', 'Front'),
  profileLeft('profile_left', 'Left profile'),
  profileRight('profile_right', 'Right profile'),
  fullBody('full_body', 'Full body'),
  costume('costume', 'Costume'),
  expressionSmile('expression_smile', 'Smile'),
  expressionTense('expression_tense', 'Tense');

  const KinetoCastImageKind(this.wireName, this.label);

  final String wireName;
  final String label;
}

final class KinetoCastImage {
  const KinetoCastImage({
    required this.id,
    required this.kind,
    required this.mediaPath,
    this.status,
    this.identityReference,
  });

  final String id;
  final KinetoCastImageKind kind;

  /// Absolute path of the image.
  final String mediaPath;

  /// A portrait's standing; null for a reference.
  final KinetoChoiceStatus? status;

  /// The portrait a reference was made from.
  final String? identityReference;
}

/// One character worth casting.
final class KinetoCastCharacter {
  const KinetoCastCharacter({
    required this.id,
    required this.name,
    this.images = const <KinetoCastImage>[],
    this.lockedPortrait,
    this.lockedReferences = const <String>[],
    this.stale = const <String>[],
    this.unresolved = false,
  });

  final String id;
  final String name;

  /// Every casting image, oldest first.
  final List<KinetoCastImage> images;

  /// The portrait of the locked cast; null when not cast.
  final String? lockedPortrait;
  final List<String> lockedReferences;

  /// The parents of the locked cast that changed since it was locked.
  final List<String> stale;

  /// An image of this character may have been charged without its result.
  final bool unresolved;

  bool get locked => lockedPortrait != null;

  Iterable<KinetoCastImage> get portraits =>
      images.where((image) => image.kind == KinetoCastImageKind.portrait);

  KinetoCastImage? get selectedPortrait {
    for (final image in portraits) {
      if (image.status == KinetoChoiceStatus.selected) return image;
    }
    return null;
  }

  /// The references made from the selected portrait, newest of each kind.
  Map<KinetoCastImageKind, KinetoCastImage> get pack {
    final selected = selectedPortrait;
    final found = <KinetoCastImageKind, KinetoCastImage>{};
    if (selected == null) return found;
    for (final image in images) {
      if (image.identityReference == selected.id) found[image.kind] = image;
    }
    return found;
  }
}

/// The bible, the style and the cast as the engine last published them.
final class KinetoProduction {
  const KinetoProduction({
    this.bible,
    this.bibleRevisions = const <KinetoRevision>[],
    this.bibleLocked,
    this.bibleStale = const <KinetoBibleStaleness>[],
    this.presets = const <KinetoStylePreset>[],
    this.samples = const <KinetoStyleSample>[],
    this.style,
    this.styleUnresolved = false,
    this.characters = const <KinetoCastCharacter>[],
    this.damaged = false,
  });

  /// The latest bible revision still in play.
  final KinetoBible? bible;

  /// Every bible revision, oldest first.
  final List<KinetoRevision> bibleRevisions;

  /// The bible revision casting and keyframes follow.
  final int? bibleLocked;
  final List<KinetoBibleStaleness> bibleStale;
  final List<KinetoStylePreset> presets;

  /// Every style sample, oldest first.
  final List<KinetoStyleSample> samples;
  final KinetoStyleLock? style;

  /// A style sample may have been charged without its result.
  final bool styleUnresolved;

  /// The characters worth casting, in bible order.
  final List<KinetoCastCharacter> characters;

  /// The bible, style or cast could not be read.
  final bool damaged;

  /// The bible the director works on is the one production follows.
  bool get bibleIsLocked => bible != null && bibleLocked == bible!.revision;
}

/// Paid images whose cost is estimated before asking.
enum KinetoImageCostKind {
  sample('sample'),
  portrait('portrait'),
  pack('pack');

  const KinetoImageCostKind(this.wireName);

  final String wireName;
}

/// What a paid image request costs, as the provider declares it.
final class KinetoImageCost {
  const KinetoImageCost({required this.micros, required this.images});

  /// USD micros for all of [images].
  final int micros;
  final int images;
}

/// Paid image work recovered when a project opened.
enum KinetoImageWork {
  styleSamples('style sample(s)'),
  casting('casting image(s)');

  const KinetoImageWork(this.label);

  final String label;
}

/// Bible, style and casting commands and reads. Every command queues on the
/// project's worker and completes when it has run; reads return published
/// state and never do I/O.
///
/// Paid image commands fail with [KinetoProjectException.paidCallUnresolved]
/// while an image of theirs may have been charged without its result, unless
/// `resubmit` is the director's decision to risk paying twice.
extension KinetoProjectProduction on KinetoProjectSession {
  /// Write a bible of the locked screenplay with the session's language
  /// model.
  Future<void> writeBible({bool resubmit = false}) =>
      _submitDocument((token) => _kinetoProjectWriteBible(_handle, resubmit, token));

  /// Save the director's [edit] of an entry of bible revision [basedOn],
  /// which must be the latest.
  Future<void> saveBibleEntry(int basedOn, KinetoBibleEntryEdit edit) {
    final contract = KinetoNativeContract.instance;
    return _submitDocument((token) {
      final arena = Arena();
      try {
        final id = _encodeUtf8(arena, edit.id);
        final description = _encodeUtf8(arena, edit.description);
        final appearance = _encodeUtf8(arena, edit.appearance);
        final wardrobe = _encodeUtf8(arena, edit.wardrobe);
        final count = edit.forbiddenChanges.length;
        final forbidden = arena<_NativeText>(count == 0 ? 1 : count);
        for (var index = 0; index < count; index++) {
          final text = _encodeUtf8(arena, edit.forbiddenChanges[index]);
          forbidden[index]
            ..pointer = text.pointer
            ..length = text.length;
        }
        return _kinetoProjectSaveBibleEntry(
          _handle,
          basedOn,
          contract.code(contract.entryKinds, edit.kind.wireName),
          id.pointer,
          id.length,
          description.pointer,
          description.length,
          appearance.pointer,
          appearance.length,
          wardrobe.pointer,
          wardrobe.length,
          forbidden,
          count,
          token,
        );
      } finally {
        arena.releaseAll();
      }
    });
  }

  /// Lock bible [revision] as the one casting and keyframes follow.
  Future<void> lockBible(int revision) =>
      _submitDocument((token) => _kinetoProjectLockBible(_handle, revision, token));

  /// One paid sample of style [preset].
  Future<void> sampleStyle(String preset, {String? note, bool resubmit = false}) =>
      _submitDocument((token) {
        final arena = Arena();
        try {
          final presetBytes = _encodeUtf8(arena, preset);
          final noteBytes = _encodeUtf8(arena, note ?? '');
          return _kinetoProjectSampleStyle(
            _handle,
            presetBytes.pointer,
            presetBytes.length,
            noteBytes.pointer,
            noteBytes.length,
            resubmit,
            token,
          );
        } finally {
          arena.releaseAll();
        }
      });

  /// One paid sample of [sampleId]'s preset, refined by [note].
  Future<void> refineStyle(String sampleId, String note, {bool resubmit = false}) =>
      _submitDocument((token) {
        final arena = Arena();
        try {
          final sample = _encodeUtf8(arena, sampleId);
          final noteBytes = _encodeUtf8(arena, note);
          return _kinetoProjectRefineStyle(
            _handle,
            sample.pointer,
            sample.length,
            noteBytes.pointer,
            noteBytes.length,
            resubmit,
            token,
          );
        } finally {
          arena.releaseAll();
        }
      });

  Future<void> selectStyle(String sampleId) => _decideStyle('select', sampleId);
  Future<void> rejectStyle(String sampleId) => _decideStyle('reject', sampleId);

  /// Lock the selected sample [sampleId] as the style every image follows.
  Future<void> lockStyle(String sampleId) => _decideStyle('lock', sampleId);

  Future<void> _decideStyle(String decision, String sampleId) {
    final contract = KinetoNativeContract.instance;
    return _submitDocument((token) {
      final arena = Arena();
      try {
        final sample = _encodeUtf8(arena, sampleId);
        return _kinetoProjectDecideStyle(
          _handle,
          contract.code(contract.decisions, decision),
          sample.pointer,
          sample.length,
          token,
        );
      } finally {
        arena.releaseAll();
      }
    });
  }

  /// Have [count] portrait candidates of [characterId] that are not rejected
  /// and follow its current entry and the locked style, paying only for
  /// those missing. [count] is how many to have, not how many to add.
  Future<void> audition(String characterId, int count, {bool resubmit = false}) =>
      _submitDocument((token) {
        final arena = Arena();
        try {
          final character = _encodeUtf8(arena, characterId);
          return _kinetoProjectAudition(
            _handle,
            character.pointer,
            character.length,
            count,
            resubmit,
            token,
          );
        } finally {
          arena.releaseAll();
        }
      });

  /// Complete the paid reference pack of [characterId]'s selected portrait,
  /// making the kinds in [remake] again although the pack holds them.
  Future<void> makePack(
    String characterId, {
    List<KinetoCastImageKind> remake = const <KinetoCastImageKind>[],
    bool resubmit = false,
  }) {
    final contract = KinetoNativeContract.instance;
    return _submitDocument((token) {
      final arena = Arena();
      try {
        final character = _encodeUtf8(arena, characterId);
        final kinds = arena<Uint32>(remake.isEmpty ? 1 : remake.length);
        for (var index = 0; index < remake.length; index++) {
          kinds[index] = contract.code(contract.castImageKinds, remake[index].wireName);
        }
        return _kinetoProjectMakePack(
          _handle,
          character.pointer,
          character.length,
          kinds,
          remake.length,
          resubmit,
          token,
        );
      } finally {
        arena.releaseAll();
      }
    });
  }

  Future<void> selectPortrait(String characterId, String portraitId) =>
      _decidePortrait('select', characterId, portraitId);

  Future<void> rejectPortrait(String characterId, String portraitId) =>
      _decidePortrait('reject', characterId, portraitId);

  Future<void> _decidePortrait(String decision, String characterId, String portraitId) {
    final contract = KinetoNativeContract.instance;
    return _submitDocument((token) {
      final arena = Arena();
      try {
        final character = _encodeUtf8(arena, characterId);
        final portrait = _encodeUtf8(arena, portraitId);
        return _kinetoProjectDecidePortrait(
          _handle,
          contract.code(contract.decisions, decision),
          character.pointer,
          character.length,
          portrait.pointer,
          portrait.length,
          token,
        );
      } finally {
        arena.releaseAll();
      }
    });
  }

  /// Lock [characterId] as cast: its selected portrait and that portrait's
  /// complete reference pack.
  Future<void> lockCharacter(String characterId) => _submitDocument((token) {
        final arena = Arena();
        try {
          final character = _encodeUtf8(arena, characterId);
          return _kinetoProjectLockCharacter(_handle, character.pointer, character.length, token);
        } finally {
          arena.releaseAll();
        }
      });

  /// What [what] costs with the session's image provider; null when it
  /// cannot make it or does not say.
  KinetoImageCost? imageCost(KinetoImageCostKind what) {
    _ensureOpen();
    final contract = KinetoNativeContract.instance;
    final arena = Arena();
    try {
      final micros = arena<Uint64>();
      final images = arena<Uint32>();
      final code = _kinetoProjectImageCost(
        _handle,
        contract.code(contract.imageCosts, what.wireName),
        micros,
        images,
      );
      if (code == KinetoProjectException.costUnknown) return null;
      _checkProjectResult(code);
      return KinetoImageCost(micros: micros.value, images: images.value);
    } finally {
      arena.releaseAll();
    }
  }

  /// The bible, the style and the cast as last published.
  KinetoProduction get production {
    _ensureOpen();
    final reader = _ProductionReader(_handle);
    try {
      return reader.read();
    } finally {
      reader.release();
    }
  }
}

/// Reads the published bible, style and cast field by field.
final class _ProductionReader {
  _ProductionReader(this._handle);

  final Pointer<Void> _handle;
  final Arena _arena = Arena();
  final KinetoNativeContract _contract = KinetoNativeContract.instance;
  late final Pointer<Uint32> _count = _arena<Uint32>();
  late final Pointer<Uint64> _value = _arena<Uint64>();

  void release() => _arena.releaseAll();

  int count(String what, [int group = 0]) {
    _checkProjectResult(
      _kinetoProjectProductionCount(
        _handle,
        _contract.code(_contract.productionCounts, what),
        group,
        _count,
      ),
    );
    return _count.value;
  }

  int value(String field, {int group = 0, int item = 0}) {
    _checkProjectResult(
      _kinetoProjectProductionValue(
        _handle,
        _contract.code(_contract.productionValues, field),
        group,
        item,
        _value,
      ),
    );
    return _value.value;
  }

  String text(String field, {int group = 0, int item = 0}) {
    final code = _contract.code(_contract.productionTexts, field);
    return _copyProjectUtf8(
      _handle,
      (session, buffer, capacity, length) => _kinetoProjectProductionTextCopy(
        session,
        code,
        group,
        item,
        buffer,
        capacity,
        length,
      ),
      field,
    );
  }

  String? optional(String field, {int group = 0, int item = 0}) {
    final found = text(field, group: group, item: item);
    return found.isEmpty ? null : found;
  }

  List<String> texts(String counted, String field, {int group = 0}) => <String>[
        for (var item = 0; item < count(counted, group); item++)
          text(field, group: group, item: item),
      ];

  T named<T extends Enum>(List<T> values, String Function(T) wireName, Map<String, int> table, int code) =>
      _byWireName(values, wireName, _contract.nameOf(table, code));

  KinetoProduction read() {
    _checkProjectResult(_kinetoProjectProductionState(_handle, _value));
    final bits = _value.value;
    int? revision(String field) {
      final value = _contract.productionField(bits, field);
      return value == 0 ? null : value;
    }

    final bibleRevision = revision('bible_revision');
    final hasStyle = _contract.productionField(bits, 'style_locked') != 0;
    return KinetoProduction(
      bible: bibleRevision == null ? null : _bible(bibleRevision),
      bibleRevisions: <KinetoRevision>[
        for (var item = 0; item < count('bible_revisions'); item++) _revision(item),
      ],
      bibleLocked: revision('bible_locked_revision'),
      bibleStale: <KinetoBibleStaleness>[
        for (var item = 0; item < count('bible_stale_causes'); item++)
          KinetoBibleStaleness(
            named(
              KinetoBibleStaleCause.values,
              (cause) => cause.wireName,
              _contract.staleCauses,
              value('bible_stale_cause', item: item),
            ),
            text('bible_stale_subject', item: item),
          ),
      ],
      presets: <KinetoStylePreset>[
        for (var item = 0; item < count('style_presets'); item++)
          KinetoStylePreset(
            id: text('preset_id', item: item),
            name: text('preset_name', item: item),
          ),
      ],
      samples: <KinetoStyleSample>[
        for (var item = 0; item < count('style_samples'); item++) _sample(item),
      ],
      style: hasStyle
          ? KinetoStyleLock(
              sampleId: text('style_lock_sample'),
              preset: text('style_lock_preset'),
              note: optional('style_lock_note'),
            )
          : null,
      styleUnresolved: _contract.productionField(bits, 'style_unresolved') != 0,
      characters: <KinetoCastCharacter>[
        for (var group = 0; group < count('cast_characters'); group++) _character(group),
      ],
      damaged: _contract.productionField(bits, 'production_damaged') != 0,
    );
  }

  KinetoBible _bible(int revision) => KinetoBible(
        revision: revision,
        setting: text('world_setting'),
        period: text('world_period'),
        visualRules: texts('world_visual_rules', 'world_visual_rule'),
        continuityRules: texts('world_continuity_rules', 'world_continuity_rule'),
        characters: <KinetoBibleEntry>[
          for (var group = 0; group < count('bible_characters'); group++)
            KinetoBibleEntry(
              kind: KinetoBibleEntryKind.character,
              id: text('character_id', group: group),
              name: text('character_name', group: group),
              description: text('character_description', group: group),
              appearance: text('character_appearance', group: group),
              wardrobe: text('character_wardrobe', group: group),
              wardrobeChanges: <KinetoWardrobeChange>[
                for (var item = 0; item < count('character_wardrobe_changes', group); item++)
                  KinetoWardrobeChange(
                    scenes: text('character_wardrobe_change_scenes', group: group, item: item),
                    wardrobe: text('character_wardrobe_change', group: group, item: item),
                  ),
              ],
              forbiddenChanges: texts('character_forbidden', 'character_forbidden', group: group),
              continuityNotes: texts('character_notes', 'character_note', group: group),
            ),
        ],
        locations: <KinetoBibleEntry>[
          for (var group = 0; group < count('bible_locations'); group++)
            KinetoBibleEntry(
              kind: KinetoBibleEntryKind.location,
              id: text('location_id', group: group),
              name: text('location_name', group: group),
              description: text('location_description', group: group),
              appearance: text('location_appearance', group: group),
              forbiddenChanges: texts('location_forbidden', 'location_forbidden', group: group),
            ),
        ],
        props: <KinetoBibleEntry>[
          for (var group = 0; group < count('bible_props'); group++)
            KinetoBibleEntry(
              kind: KinetoBibleEntryKind.prop,
              id: text('prop_id', group: group),
              name: text('prop_name', group: group),
              description: text('prop_description', group: group),
              appearance: text('prop_appearance', group: group),
              forbiddenChanges: texts('prop_forbidden', 'prop_forbidden', group: group),
            ),
        ],
      );

  KinetoRevision _revision(int item) {
    final basedOn = value('bible_revision_based_on', item: item);
    return KinetoRevision(
      number: value('bible_revision_number', item: item),
      status: named(
        KinetoRevisionStatus.values,
        (status) => status.wireName,
        _contract.revisionStatuses,
        value('bible_revision_status', item: item),
      ),
      origin: named(
        KinetoRevisionOrigin.values,
        (origin) => origin.wireName,
        _contract.origins,
        value('bible_revision_origin', item: item),
      ),
      basedOn: basedOn == 0 ? null : basedOn,
      provider: text('bible_revision_provider', item: item),
      model: text('bible_revision_model', item: item),
    );
  }

  KinetoStyleSample _sample(int item) => KinetoStyleSample(
        id: text('sample_id', item: item),
        preset: text('sample_preset', item: item),
        sceneId: text('sample_scene', item: item),
        status: named(
          KinetoChoiceStatus.values,
          (status) => status.wireName,
          _contract.choiceStatuses,
          value('sample_status', item: item),
        ),
        mediaPath: text('sample_media', item: item),
        note: optional('sample_note', item: item),
        refines: optional('sample_refines', item: item),
        provider: text('sample_provider', item: item),
        model: text('sample_model', item: item),
      );

  KinetoCastCharacter _character(int group) => KinetoCastCharacter(
        id: text('cast_character_id', group: group),
        name: text('cast_character_name', group: group),
        images: <KinetoCastImage>[
          for (var item = 0; item < count('cast_images', group); item++) _image(group, item),
        ],
        lockedPortrait: value('cast_locked', group: group) == 0
            ? null
            : text('cast_lock_portrait', group: group),
        lockedReferences: texts('cast_lock_references', 'cast_lock_reference', group: group),
        stale: texts('cast_stale', 'cast_stale', group: group),
        unresolved: value('cast_unresolved', group: group) != 0,
      );

  KinetoCastImage _image(int group, int item) {
    final status = value('cast_image_status', group: group, item: item);
    return KinetoCastImage(
      id: text('cast_image_id', group: group, item: item),
      kind: named(
        KinetoCastImageKind.values,
        (kind) => kind.wireName,
        _contract.castImageKinds,
        value('cast_image_kind', group: group, item: item),
      ),
      mediaPath: text('cast_image_media', group: group, item: item),
      status: status == 0
          ? null
          : named(
              KinetoChoiceStatus.values,
              (choice) => choice.wireName,
              _contract.choiceStatuses,
              status,
            ),
      identityReference: optional('cast_image_identity_reference', group: group, item: item),
    );
  }
}
