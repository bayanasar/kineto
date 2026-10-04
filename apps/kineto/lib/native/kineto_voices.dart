part of 'kineto_engine.dart';

@Native<Int32 Function(Pointer<Void>, Uint64)>(symbol: 'kineto_project_load_voices')
external int _kinetoProjectLoadVoices(Pointer<Void> session, int token);

/// One voice to audition, laid out as `KinetoVoiceChoice`.
final class _NativeVoiceChoice extends Struct {
  external _NativeText voice;
  external _NativeText name;

  @Uint32()
  external int emotion;

  @Uint32()
  external int speedPermille;
}

/// One preference, laid out as `KinetoPreference`.
final class _NativePreference extends Struct {
  @Uint32()
  external int dimension;

  @Uint32()
  external int stance;

  external _NativeText value;

  @Uint32()
  external int strengthPermille;

  @Uint32()
  external int keptFrom;
}

@Native<
  Int32 Function(
    Pointer<Void>,
    Pointer<Uint8>,
    Uint64,
    Pointer<_NativeVoiceChoice>,
    Uint64,
    Pointer<Uint8>,
    Uint64,
    Bool,
    Uint64,
  )
>(symbol: 'kineto_project_audition_voices')
external int _kinetoProjectAuditionVoices(
  Pointer<Void> session,
  Pointer<Uint8> character,
  int characterLength,
  Pointer<_NativeVoiceChoice> choices,
  int choiceCount,
  Pointer<Uint8> line,
  int lineLength,
  bool resubmit,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Pointer<Uint8>, Uint64, Pointer<Uint8>, Uint64, Uint64)>(
  symbol: 'kineto_project_decide_voice',
)
external int _kinetoProjectDecideVoice(
  Pointer<Void> session,
  int decision,
  Pointer<Uint8> character,
  int characterLength,
  Pointer<Uint8> audition,
  int auditionLength,
  int token,
);

@Native<
  Int32 Function(
    Pointer<Void>,
    Pointer<Uint8>,
    Uint64,
    Uint32,
    Uint32,
    Uint32,
    Uint32,
    Pointer<Uint8>,
    Uint64,
    Bool,
    Uint64,
  )
>(symbol: 'kineto_project_dub_line')
external int _kinetoProjectDubLine(
  Pointer<Void> session,
  Pointer<Uint8> scene,
  int sceneLength,
  int line,
  int takes,
  int delivery,
  int speedPermille,
  Pointer<Uint8> note,
  int noteLength,
  bool resubmit,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Pointer<Uint8>, Uint64, Pointer<Uint8>, Uint64, Uint32, Uint64)>(
  symbol: 'kineto_project_decide_take',
)
external int _kinetoProjectDecideTake(
  Pointer<Void> session,
  int decision,
  Pointer<Uint8> scene,
  int sceneLength,
  Pointer<Uint8> take,
  int takeLength,
  int line,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Pointer<_NativePreference>, Uint64, Uint64)>(
  symbol: 'kineto_project_save_preferences',
)
external int _kinetoProjectSavePreferences(
  Pointer<Void> session,
  int basedOn,
  Pointer<_NativePreference> entries,
  int entryCount,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Bool, Uint64)>(symbol: 'kineto_project_suggest_preferences')
external int _kinetoProjectSuggestPreferences(Pointer<Void> session, bool resubmit, int token);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Uint32, Pointer<Uint8>, Uint64, Uint64)>(
  symbol: 'kineto_project_decide_suggestion',
)
external int _kinetoProjectDecideSuggestion(
  Pointer<Void> session,
  int decision,
  int batch,
  int index,
  Pointer<Uint8> value,
  int valueLength,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Bool, Uint64)>(symbol: 'kineto_project_recommend_voices')
external int _kinetoProjectRecommendVoices(Pointer<Void> session, bool resubmit, int token);

@Native<Int32 Function(Pointer<Void>, Pointer<Uint8>, Uint64, Uint32, Pointer<Uint64>)>(
  symbol: 'kineto_project_speech_cost',
)
external int _kinetoProjectSpeechCost(
  Pointer<Void> session,
  Pointer<Uint8> text,
  int textLength,
  int count,
  Pointer<Uint64> micros,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Pointer<Uint32>)>(symbol: 'kineto_project_voice_count')
external int _kinetoProjectVoiceCount(Pointer<Void> session, int what, int group, Pointer<Uint32> count);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Uint32, Pointer<Uint64>)>(
  symbol: 'kineto_project_voice_value',
)
external int _kinetoProjectVoiceValue(
  Pointer<Void> session,
  int field,
  int group,
  int item,
  Pointer<Uint64> value,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Uint32, Pointer<Uint8>, Uint64, Pointer<Uint64>)>(
  symbol: 'kineto_project_voice_text_copy',
)
external int _kinetoProjectVoiceTextCopy(
  Pointer<Void> session,
  int field,
  int group,
  int item,
  Pointer<Uint8> buffer,
  int capacity,
  Pointer<Uint64> length,
);

/// A delivery a line or an audition can be asked for.
enum KinetoEmotion {
  happy('happy', 'Happy'),
  sad('sad', 'Sad'),
  angry('angry', 'Angry'),
  fearful('fearful', 'Fearful'),
  disgusted('disgusted', 'Disgusted'),
  surprised('surprised', 'Surprised'),
  calm('calm', 'Calm'),
  fluent('fluent', 'Fluent'),
  whisper('whisper', 'Whisper');

  const KinetoEmotion(this.wireName, this.label);

  final String wireName;
  final String label;
}

/// What a preference is about.
enum KinetoDimension {
  cameraMotion('camera_motion', 'Camera motion'),
  lighting('lighting', 'Lighting'),
  framing('framing', 'Framing'),
  lens('lens', 'Lens'),
  color('color', 'Colour'),
  composition('composition', 'Composition'),
  performance('performance', 'Performance'),
  pacing('pacing', 'Pacing');

  const KinetoDimension(this.wireName, this.label);

  final String wireName;
  final String label;
}

enum KinetoStance {
  prefer('prefer', 'Prefer'),
  avoid('avoid', 'Avoid');

  const KinetoStance(this.wireName, this.label);

  final String wireName;
  final String label;
}

/// Where a line's length comes from.
enum KinetoDurationSource {
  /// Worked out from its words and its speaker's rate.
  estimated('estimated'),

  /// The length of its locked take.
  measured('measured');

  const KinetoDurationSource(this.wireName);

  final String wireName;
}

enum KinetoSpeechProviderKind {
  localDemo('local_demo', 'Local demo'),
  minimax('minimax', 'MiniMax'),
  plugin('plugin', 'Plugin');

  const KinetoSpeechProviderKind(this.wireName, this.label);

  final String wireName;
  final String label;
}

/// A voice the speech provider offers.
final class KinetoVoice {
  const KinetoVoice({required this.id, required this.name, this.description = ''});

  final String id;
  final String name;
  final String description;
}

/// A voice to audition, with how it should speak.
final class KinetoVoiceChoice {
  const KinetoVoiceChoice({required this.voiceId, required this.name, this.emotion, this.speedPermille});

  final String voiceId;
  final String name;

  /// The delivery to audition it with; null for the voice's own.
  final KinetoEmotion? emotion;

  /// The rate, in thousandths of normal; null for normal.
  final int? speedPermille;
}

/// One audition: a voice saying a line as a character.
final class KinetoAudition {
  const KinetoAudition({
    required this.id,
    required this.status,
    required this.voiceId,
    required this.voiceName,
    required this.line,
    required this.mediaPath,
    required this.durationMillis,
    this.emotion,
    this.speedPermille = 1000,
  });

  final String id;
  final KinetoChoiceStatus status;
  final String voiceId;
  final String voiceName;
  final String line;

  /// Absolute path of its audio.
  final String mediaPath;
  final int durationMillis;
  final String? emotion;
  final int speedPermille;
}

/// A voice a model recommends for a character.
final class KinetoRecommendedVoice {
  const KinetoRecommendedVoice({required this.voiceId, required this.reason, this.name});

  final String voiceId;
  final String? name;
  final String reason;
}

/// Where casting one character's voice stands.
final class KinetoCharacterVoice {
  const KinetoCharacterVoice({
    required this.characterId,
    required this.characterName,
    required this.defaultLine,
    this.auditions = const <KinetoAudition>[],
    this.lockedAudition,
    this.lockedVoiceId,
    this.lockedVoiceName,
    this.stale = const <String>[],
    this.unresolved = false,
    this.recommended = const <KinetoRecommendedVoice>[],
  });

  final String characterId;
  final String characterName;

  /// What an audition says unless the director writes a line.
  final String defaultLine;
  final List<KinetoAudition> auditions;

  /// The audition locked as the character's voice.
  final String? lockedAudition;
  final String? lockedVoiceId;
  final String? lockedVoiceName;

  /// What the locked voice was made from that changed since.
  final List<String> stale;

  /// An audition may have been charged without its audio.
  final bool unresolved;

  /// The voices a model last recommended, best first.
  final List<KinetoRecommendedVoice> recommended;

  bool get locked => lockedAudition != null;
}

/// One take of a line.
final class KinetoLineTake {
  const KinetoLineTake({
    required this.id,
    required this.status,
    required this.mediaPath,
    required this.durationMillis,
    this.note,
    this.emotion,
    this.speedPermille = 1000,
  });

  final String id;
  final KinetoChoiceStatus status;

  /// Absolute path of its audio.
  final String mediaPath;
  final int durationMillis;
  final String? note;
  final String? emotion;
  final int speedPermille;
}

/// A line of the screenplay and where its dubbing stands.
final class KinetoLine {
  const KinetoLine({
    required this.sceneId,
    required this.number,
    required this.text,
    this.characterId,
    this.parenthetical,
    this.takes = const <KinetoLineTake>[],
    this.lockedTake,
    this.stale = const <String>[],
    this.unresolved = false,
    this.takeCostMicros,
    this.durationMillis = 0,
    this.durationSource = KinetoDurationSource.estimated,
  });

  final String sceneId;

  /// Its place among the scene's lines, from 1.
  final int number;
  final String text;
  final String? characterId;
  final String? parenthetical;
  final List<KinetoLineTake> takes;

  /// The take the line is locked to.
  final String? lockedTake;

  /// Why the locked take no longer fits.
  final List<String> stale;

  /// A take may have been charged without its audio.
  final bool unresolved;

  /// What one take costs with the session's speech provider; null when it
  /// does not say.
  final int? takeCostMicros;

  /// How long it takes to say.
  final int durationMillis;
  final KinetoDurationSource durationSource;

  bool get locked => lockedTake != null;
}

/// One entry of the director's preference profile.
final class KinetoPreference {
  const KinetoPreference({
    required this.dimension,
    required this.stance,
    required this.value,
    this.strengthPermille,
    this.keptFrom = 0,
    this.reason,
    this.evidence = const <String>[],
  });

  final KinetoDimension dimension;
  final KinetoStance stance;

  /// What, in the director's words, such as `slow dolly`.
  final String value;

  /// How strongly, in thousandths, when the director says.
  final int? strengthPermille;

  /// The entry of the profile in force, from 1, whose source it keeps; 0
  /// for the director's own. Entries read back keep their own place.
  final int keptFrom;

  /// Why, for one accepted from a suggestion.
  final String? reason;

  /// The candidates it was drawn from, for one accepted from a suggestion.
  final List<String> evidence;

  bool get fromSuggestion => reason != null;

  KinetoPreference copyWith({
    KinetoDimension? dimension,
    KinetoStance? stance,
    String? value,
    int? Function()? strengthPermille,
  }) =>
      KinetoPreference(
        dimension: dimension ?? this.dimension,
        stance: stance ?? this.stance,
        value: value ?? this.value,
        strengthPermille: strengthPermille == null ? this.strengthPermille : strengthPermille(),
        keptFrom: keptFrom,
        reason: reason,
        evidence: evidence,
      );
}

/// A preference a model suggests from the director's choices.
final class KinetoSuggestion {
  const KinetoSuggestion({
    required this.batch,
    required this.index,
    required this.dimension,
    required this.stance,
    required this.value,
    required this.reason,
    this.evidence = const <String>[],
    this.comparedImages = false,
  });

  final int batch;
  final int index;
  final KinetoDimension dimension;
  final KinetoStance stance;
  final String value;
  final String reason;
  final List<String> evidence;

  /// The model saw the images, not only their prompts and notes.
  final bool comparedImages;
}

/// A preference profile revision.
final class KinetoPreferenceRevision {
  const KinetoPreferenceRevision({required this.number, required this.origin});

  final int number;
  final KinetoRevisionOrigin origin;
}

/// The voices, lines and preferences as last published.
final class KinetoVoices {
  const KinetoVoices({
    this.speechProvider = KinetoSpeechProviderKind.localDemo,
    this.speechReady = true,
    this.characterCostMicros,
    this.voices = const <KinetoVoice>[],
    this.voicesProblem,
    this.characters = const <KinetoCharacterVoice>[],
    this.lines = const <KinetoLine>[],
    this.preferences = const <KinetoPreference>[],
    this.preferencesRevision,
    this.preferenceRevisions = const <KinetoPreferenceRevision>[],
    this.suggestions = const <KinetoSuggestion>[],
    this.unconsideredChoices = 0,
    this.suggestionsReady = false,
    this.planPreferencesChanged = false,
  });

  final KinetoSpeechProviderKind speechProvider;

  /// Whether speech can be asked for: a paid provider needs credentials.
  final bool speechReady;

  /// What a character of text costs; null when the provider does not say.
  final int? characterCostMicros;

  /// The voices the provider offers, once listed.
  final List<KinetoVoice> voices;

  /// Why the voices could not be listed.
  final String? voicesProblem;

  /// Each character worth casting, in cast order.
  final List<KinetoCharacterVoice> characters;

  /// Every line of the screenplay work builds on.
  final List<KinetoLine> lines;

  /// The profile in force, and its revision.
  final List<KinetoPreference> preferences;
  final int? preferencesRevision;
  final List<KinetoPreferenceRevision> preferenceRevisions;

  /// Suggestions the director has not reviewed, oldest first.
  final List<KinetoSuggestion> suggestions;

  /// Choices no suggestions have considered yet, and whether they are
  /// enough to ask.
  final int unconsideredChoices;
  final bool suggestionsReady;

  /// The plan in play was made with other preferences than those in force.
  final bool planPreferencesChanged;

  /// The voice of [characterId], if it is worth casting.
  KinetoCharacterVoice? character(String characterId) {
    for (final character in characters) {
      if (character.characterId == characterId) return character;
    }
    return null;
  }

  /// The lines of [sceneId], in order.
  List<KinetoLine> linesOf(String sceneId) => <KinetoLine>[
        for (final line in lines)
          if (line.sceneId == sceneId) line,
      ];
}

extension KinetoProjectVoices on KinetoProjectSession {
  /// Ask the speech provider which voices it offers. Free.
  Future<void> loadVoices() => _submitDocument((token) => _kinetoProjectLoadVoices(_handle, token));

  /// Have each of [choices] say [line] (null for the character's default
  /// line) as [characterId]. Paid; a voice already heard saying it is not
  /// asked again.
  Future<void> auditionVoices(
    String characterId,
    List<KinetoVoiceChoice> choices, {
    String? line,
    bool resubmit = false,
  }) {
    final contract = KinetoNativeContract.instance;
    return _submitDocument((token) {
      final arena = Arena();
      try {
        final character = _encodeUtf8(arena, characterId);
        final said = _encodeUtf8(arena, line ?? '');
        final native = arena<_NativeVoiceChoice>(choices.isEmpty ? 1 : choices.length);
        for (var index = 0; index < choices.length; index++) {
          final choice = choices[index];
          final voice = _encodeUtf8(arena, choice.voiceId);
          final name = _encodeUtf8(arena, choice.name);
          final slot = native[index];
          slot.voice
            ..pointer = voice.pointer
            ..length = voice.length;
          slot.name
            ..pointer = name.pointer
            ..length = name.length;
          slot.emotion = choice.emotion == null ? 0 : contract.code(contract.emotions, choice.emotion!.wireName);
          slot.speedPermille = choice.speedPermille ?? 0;
        }
        return _kinetoProjectAuditionVoices(
          _handle,
          character.pointer,
          character.length,
          native,
          choices.length,
          said.pointer,
          said.length,
          resubmit,
          token,
        );
      } finally {
        arena.releaseAll();
      }
    });
  }

  Future<void> selectVoice(String characterId, String auditionId) =>
      _decideVoice('select', characterId, auditionId);

  Future<void> rejectVoice(String characterId, String auditionId) =>
      _decideVoice('reject', characterId, auditionId);

  /// Lock [characterId]'s selected audition as its voice.
  Future<void> lockVoice(String characterId) => _decideVoice('lock', characterId, '');

  Future<void> _decideVoice(String decision, String characterId, String auditionId) {
    final contract = KinetoNativeContract.instance;
    return _submitDocument((token) {
      final arena = Arena();
      try {
        final character = _encodeUtf8(arena, characterId);
        final audition = _encodeUtf8(arena, auditionId);
        return _kinetoProjectDecideVoice(
          _handle,
          contract.code(contract.decisions, decision),
          character.pointer,
          character.length,
          audition.pointer,
          audition.length,
          token,
        );
      } finally {
        arena.releaseAll();
      }
    });
  }

  /// Have [takes] takes of line [line] (from 1) of [sceneId] spoken in its
  /// character's locked voice. Paid.
  Future<void> dubLine(
    String sceneId,
    int line, {
    int takes = 1,
    KinetoEmotion? delivery,
    int? speedPermille,
    String? note,
    bool resubmit = false,
  }) {
    final contract = KinetoNativeContract.instance;
    return _submitDocument((token) {
      final arena = Arena();
      try {
        final scene = _encodeUtf8(arena, sceneId);
        final noted = _encodeUtf8(arena, note ?? '');
        return _kinetoProjectDubLine(
          _handle,
          scene.pointer,
          scene.length,
          line,
          takes,
          delivery == null ? 0 : contract.code(contract.emotions, delivery.wireName),
          speedPermille ?? 0,
          noted.pointer,
          noted.length,
          resubmit,
          token,
        );
      } finally {
        arena.releaseAll();
      }
    });
  }

  Future<void> selectTake(String sceneId, String takeId) => _decideTake('select', sceneId, takeId, 0);

  Future<void> rejectTake(String sceneId, String takeId) => _decideTake('reject', sceneId, takeId, 0);

  /// Lock line [line] of [sceneId] to its selected take.
  Future<void> lockLine(String sceneId, int line) => _decideTake('lock', sceneId, '', line);

  Future<void> _decideTake(String decision, String sceneId, String takeId, int line) {
    final contract = KinetoNativeContract.instance;
    return _submitDocument((token) {
      final arena = Arena();
      try {
        final scene = _encodeUtf8(arena, sceneId);
        final take = _encodeUtf8(arena, takeId);
        return _kinetoProjectDecideTake(
          _handle,
          contract.code(contract.decisions, decision),
          scene.pointer,
          scene.length,
          take.pointer,
          take.length,
          line,
          token,
        );
      } finally {
        arena.releaseAll();
      }
    });
  }

  /// Save [entries] as the director's preference profile, the next
  /// revision after [basedOn] (null for the first).
  Future<void> savePreferences(int? basedOn, List<KinetoPreference> entries) {
    final contract = KinetoNativeContract.instance;
    return _submitDocument((token) {
      final arena = Arena();
      try {
        final native = arena<_NativePreference>(entries.isEmpty ? 1 : entries.length);
        for (var index = 0; index < entries.length; index++) {
          final entry = entries[index];
          final value = _encodeUtf8(arena, entry.value);
          final slot = native[index];
          slot.dimension = contract.code(contract.dimensions, entry.dimension.wireName);
          slot.stance = contract.code(contract.stances, entry.stance.wireName);
          slot.value
            ..pointer = value.pointer
            ..length = value.length;
          slot.strengthPermille = entry.strengthPermille ?? 0;
          slot.keptFrom = entry.keptFrom;
        }
        return _kinetoProjectSavePreferences(_handle, basedOn ?? 0, native, entries.length, token);
      } finally {
        arena.releaseAll();
      }
    });
  }

  /// Ask the language model what the director's choices show.
  Future<void> suggestPreferences({bool resubmit = false}) =>
      _submitDocument((token) => _kinetoProjectSuggestPreferences(_handle, resubmit, token));

  /// Take [suggestion] into the profile, as suggested or, with [value], in
  /// the director's words.
  Future<void> acceptSuggestion(KinetoSuggestion suggestion, {String? value}) =>
      _decideSuggestion('accept', suggestion, value ?? '');

  Future<void> rejectSuggestion(KinetoSuggestion suggestion) => _decideSuggestion('reject', suggestion, '');

  Future<void> _decideSuggestion(String decision, KinetoSuggestion suggestion, String value) {
    final contract = KinetoNativeContract.instance;
    return _submitDocument((token) {
      final arena = Arena();
      try {
        final words = _encodeUtf8(arena, value);
        return _kinetoProjectDecideSuggestion(
          _handle,
          contract.code(contract.suggestionDecisions, decision),
          suggestion.batch,
          suggestion.index,
          words.pointer,
          words.length,
          token,
        );
      } finally {
        arena.releaseAll();
      }
    });
  }

  /// Ask the language model which of the provider's voices fit the
  /// characters who speak.
  Future<void> recommendVoices({bool resubmit = false}) =>
      _submitDocument((token) => _kinetoProjectRecommendVoices(_handle, resubmit, token));

  /// What saying [text] [count] times costs with the speech provider, in
  /// USD micros; null when it does not say.
  int? speechCostMicros(String text, {int count = 1}) {
    _ensureOpen();
    final arena = Arena();
    try {
      final said = _encodeUtf8(arena, text);
      final micros = arena<Uint64>();
      final code = _kinetoProjectSpeechCost(_handle, said.pointer, said.length, count, micros);
      if (code == KinetoProjectException.costUnknown) return null;
      _checkProjectResult(code);
      return micros.value;
    } finally {
      arena.releaseAll();
    }
  }

  /// The voices, lines and preferences as last published.
  KinetoVoices get voices {
    _ensureOpen();
    final reader = _VoiceReader(_handle);
    try {
      return reader.read();
    } finally {
      reader.release();
    }
  }
}

/// Reads the published voices, lines and preferences field by field.
final class _VoiceReader {
  _VoiceReader(this._handle);

  final Pointer<Void> _handle;
  final Arena _arena = Arena();
  final KinetoNativeContract _contract = KinetoNativeContract.instance;
  late final Pointer<Uint32> _count = _arena<Uint32>();
  late final Pointer<Uint64> _value = _arena<Uint64>();

  void release() => _arena.releaseAll();

  int count(String what, [int group = 0]) {
    _checkProjectResult(
      _kinetoProjectVoiceCount(_handle, _contract.code(_contract.voiceCounts, what), group, _count),
    );
    return _count.value;
  }

  int value(String field, {int group = 0, int item = 0}) {
    _checkProjectResult(
      _kinetoProjectVoiceValue(_handle, _contract.code(_contract.voiceValues, field), group, item, _value),
    );
    return _value.value;
  }

  String text(String field, {int group = 0, int item = 0}) {
    final code = _contract.code(_contract.voiceTexts, field);
    return _copyProjectUtf8(
      _handle,
      (session, buffer, capacity, length) =>
          _kinetoProjectVoiceTextCopy(session, code, group, item, buffer, capacity, length),
      field,
    );
  }

  String? optional(String field, {int group = 0, int item = 0}) {
    final found = text(field, group: group, item: item);
    return found.isEmpty ? null : found;
  }

  List<String> texts(String counted, String field, {int group = 0}) => <String>[
        for (var item = 0; item < count(counted, group); item++) text(field, group: group, item: item),
      ];

  T named<T extends Enum>(List<T> values, String Function(T) wireName, Map<String, int> table, int code) =>
      _byWireName(values, wireName, _contract.nameOf(table, code));

  /// A cast character's ID or name, read with the production fields.
  String _castText(String field, int group) {
    final code = _contract.code(_contract.productionTexts, field);
    return _copyProjectUtf8(
      _handle,
      (session, buffer, capacity, length) =>
          _kinetoProjectProductionTextCopy(session, code, group, 0, buffer, capacity, length),
      field,
    );
  }

  int _castCount() {
    _checkProjectResult(
      _kinetoProjectProductionCount(
        _handle,
        _contract.code(_contract.productionCounts, 'cast_characters'),
        0,
        _count,
      ),
    );
    return _count.value;
  }

  KinetoVoices read() {
    final costKnown = value('speech_character_cost_known') != 0;
    final revision = value('preferences_revision');
    return KinetoVoices(
      speechProvider: named(
        KinetoSpeechProviderKind.values,
        (kind) => kind.wireName,
        _contract.speechProviders,
        value('speech_provider'),
      ),
      speechReady: value('speech_ready') != 0,
      characterCostMicros: costKnown ? value('speech_character_cost_micros') : null,
      voices: <KinetoVoice>[
        for (var item = 0; item < count('voices'); item++)
          KinetoVoice(
            id: text('voice_id', item: item),
            name: text('voice_name', item: item),
            description: text('voice_description', item: item),
          ),
      ],
      voicesProblem: optional('voices_problem'),
      characters: <KinetoCharacterVoice>[
        for (var group = 0; group < _castCount(); group++) _character(group),
      ],
      lines: <KinetoLine>[
        for (var group = 0; group < count('lines'); group++) _line(group),
      ],
      preferences: <KinetoPreference>[
        for (var group = 0; group < count('preferences'); group++) _preference(group),
      ],
      preferencesRevision: revision == 0 ? null : revision,
      preferenceRevisions: <KinetoPreferenceRevision>[
        for (var item = 0; item < count('preference_revisions'); item++)
          KinetoPreferenceRevision(
            number: value('preference_revision_number', item: item),
            origin: named(
              KinetoRevisionOrigin.values,
              (origin) => origin.wireName,
              _contract.origins,
              value('preference_revision_origin', item: item),
            ),
          ),
      ],
      suggestions: <KinetoSuggestion>[
        for (var group = 0; group < count('suggestions'); group++) _suggestion(group),
      ],
      unconsideredChoices: value('unconsidered_choices'),
      suggestionsReady: value('suggestions_ready') != 0,
      planPreferencesChanged: value('plan_preferences_changed') != 0,
    );
  }

  KinetoChoiceStatus _status(String field, int group, int item) => named(
        KinetoChoiceStatus.values,
        (status) => status.wireName,
        _contract.choiceStatuses,
        value(field, group: group, item: item),
      );

  KinetoCharacterVoice _character(int group) {
    final locked = value('voice_locked', group: group) != 0;
    return KinetoCharacterVoice(
      characterId: _castText('cast_character_id', group),
      characterName: _castText('cast_character_name', group),
      defaultLine: text('voice_default_line', group: group),
      auditions: <KinetoAudition>[
        for (var item = 0; item < count('voice_auditions', group); item++)
          KinetoAudition(
            id: text('audition_id', group: group, item: item),
            status: _status('audition_status', group, item),
            voiceId: text('audition_voice_id', group: group, item: item),
            voiceName: text('audition_voice_name', group: group, item: item),
            line: text('audition_line', group: group, item: item),
            mediaPath: text('audition_media', group: group, item: item),
            durationMillis: value('audition_duration_millis', group: group, item: item),
            emotion: optional('audition_emotion', group: group, item: item),
            speedPermille: value('audition_speed_permille', group: group, item: item),
          ),
      ],
      lockedAudition: locked ? text('voice_lock_audition', group: group) : null,
      lockedVoiceId: locked ? text('voice_lock_voice_id', group: group) : null,
      lockedVoiceName: locked ? text('voice_lock_voice_name', group: group) : null,
      stale: texts('voice_stale', 'voice_stale', group: group),
      unresolved: value('voice_unresolved', group: group) != 0,
      recommended: <KinetoRecommendedVoice>[
        for (var item = 0; item < count('voice_recommended', group); item++)
          KinetoRecommendedVoice(
            voiceId: text('recommended_voice_id', group: group, item: item),
            name: optional('recommended_voice_name', group: group, item: item),
            reason: text('recommended_reason', group: group, item: item),
          ),
      ],
    );
  }

  KinetoLine _line(int group) {
    final locked = value('line_locked', group: group) != 0;
    final costKnown = value('line_take_cost_known', group: group) != 0;
    final source = value('line_duration_source', group: group);
    return KinetoLine(
      sceneId: text('line_scene', group: group),
      number: value('line_number', group: group),
      text: text('line_text', group: group),
      characterId: optional('line_character', group: group),
      parenthetical: optional('line_parenthetical', group: group),
      takes: <KinetoLineTake>[
        for (var item = 0; item < count('line_takes', group); item++)
          KinetoLineTake(
            id: text('take_id', group: group, item: item),
            status: _status('take_status', group, item),
            mediaPath: text('take_media', group: group, item: item),
            durationMillis: value('take_duration_millis', group: group, item: item),
            note: optional('take_note', group: group, item: item),
            emotion: optional('take_emotion', group: group, item: item),
            speedPermille: value('take_speed_permille', group: group, item: item),
          ),
      ],
      lockedTake: locked ? text('line_lock_take', group: group) : null,
      stale: texts('line_stale', 'line_stale', group: group),
      unresolved: value('line_unresolved', group: group) != 0,
      takeCostMicros: costKnown ? value('line_take_cost_micros', group: group) : null,
      durationMillis: value('line_duration_millis', group: group),
      durationSource: named(
        KinetoDurationSource.values,
        (source) => source.wireName,
        _contract.durationSources,
        source,
      ),
    );
  }

  KinetoPreference _preference(int group) {
    final fromSuggestion = value('preference_from_suggestion', group: group) != 0;
    final strength = value('preference_strength_permille', group: group);
    return KinetoPreference(
      dimension: named(
        KinetoDimension.values,
        (dimension) => dimension.wireName,
        _contract.dimensions,
        value('preference_dimension', group: group),
      ),
      stance: named(
        KinetoStance.values,
        (stance) => stance.wireName,
        _contract.stances,
        value('preference_stance', group: group),
      ),
      value: text('preference_value', group: group),
      strengthPermille: strength == 0 ? null : strength,
      keptFrom: group + 1,
      reason: fromSuggestion ? text('preference_reason', group: group) : null,
      evidence: texts('preference_evidence', 'preference_evidence', group: group),
    );
  }

  KinetoSuggestion _suggestion(int group) => KinetoSuggestion(
        batch: value('suggestion_batch', group: group),
        index: value('suggestion_index', group: group),
        dimension: named(
          KinetoDimension.values,
          (dimension) => dimension.wireName,
          _contract.dimensions,
          value('suggestion_dimension', group: group),
        ),
        stance: named(
          KinetoStance.values,
          (stance) => stance.wireName,
          _contract.stances,
          value('suggestion_stance', group: group),
        ),
        value: text('suggestion_value', group: group),
        reason: text('suggestion_reason', group: group),
        evidence: texts('suggestion_evidence', 'suggestion_evidence', group: group),
        comparedImages: value('suggestion_compared_images', group: group) != 0,
      );
}
