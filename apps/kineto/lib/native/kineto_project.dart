part of 'kineto_engine.dart';

typedef _DestroyProjectNative = Void Function(Pointer<Void>);
typedef _ProjectTextCopyDart = int Function(
  Pointer<Void>,
  Pointer<Uint8>,
  int,
  Pointer<Uint64>,
);
typedef _PostCObject = NativeFunction<Int8 Function(Int64, Pointer<Dart_CObject>)>;

@Native<
    Int32 Function(
      Pointer<Uint8>,
      Uint64,
      Pointer<Uint8>,
      Uint64,
      Pointer<Uint8>,
      Uint64,
      Pointer<Uint8>,
      Uint64,
      Pointer<Uint8>,
      Uint64,
      Pointer<Uint8>,
      Uint64,
      Pointer<_PostCObject>,
      Int64,
      Pointer<Pointer<Void>>,
    )>(symbol: 'kineto_project_create_text')
external int _kinetoProjectCreateText(
  Pointer<Uint8> path,
  int pathLength,
  Pointer<Uint8> projectId,
  int projectIdLength,
  Pointer<Uint8> title,
  int titleLength,
  Pointer<Uint8> createdAt,
  int createdAtLength,
  Pointer<Uint8> language,
  int languageLength,
  Pointer<Uint8> source,
  int sourceLength,
  Pointer<_PostCObject> post,
  int port,
  Pointer<Pointer<Void>> outSession,
);

@Native<
    Int32 Function(
      Pointer<Uint8>,
      Uint64,
      Pointer<_PostCObject>,
      Int64,
      Pointer<Pointer<Void>>,
    )>(symbol: 'kineto_project_open')
external int _kinetoProjectOpen(
  Pointer<Uint8> path,
  int pathLength,
  Pointer<_PostCObject> post,
  int port,
  Pointer<Pointer<Void>> outSession,
);

@Native<_DestroyProjectNative>(symbol: 'kineto_project_destroy')
external void _kinetoProjectDestroy(Pointer<Void> session);

@Native<Uint32 Function(Pointer<Void>)>(
  symbol: 'kineto_project_format_version',
  isLeaf: true,
)
external int _kinetoProjectFormatVersion(Pointer<Void> session);

@Native<Uint8 Function(Pointer<Void>)>(
  symbol: 'kineto_project_is_read_only',
  isLeaf: true,
)
external int _kinetoProjectIsReadOnly(Pointer<Void> session);

@Native<Int32 Function(Pointer<Void>, Pointer<Uint8>, Uint64, Pointer<Uint64>)>(
  symbol: 'kineto_project_id_copy',
)
external int _kinetoProjectIdCopy(
  Pointer<Void> session,
  Pointer<Uint8> outBuffer,
  int outCapacity,
  Pointer<Uint64> outLength,
);

@Native<Int32 Function(Pointer<Void>, Pointer<Uint8>, Uint64, Pointer<Uint64>)>(
  symbol: 'kineto_project_title_copy',
)
external int _kinetoProjectTitleCopy(
  Pointer<Void> session,
  Pointer<Uint8> outBuffer,
  int outCapacity,
  Pointer<Uint64> outLength,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Pointer<Uint64>)>(
  symbol: 'kineto_project_shot_state',
)
external int _kinetoProjectShotState(
  Pointer<Void> session,
  int shotIndex,
  Pointer<Uint64> outState,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Pointer<Uint8>)>(
  symbol: 'kineto_project_shot_keyframes_unresolved',
)
external int _kinetoProjectShotKeyframesUnresolved(
  Pointer<Void> session,
  int shotIndex,
  Pointer<Uint8> outUnresolved,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Uint64)>(
  symbol: 'kineto_project_shot_set_direction',
)
external int _kinetoProjectShotSetDirection(
  Pointer<Void> session,
  int shotIndex,
  int direction,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint64)>(
  symbol: 'kineto_project_shot_generate',
)
external int _kinetoProjectShotGenerate(
  Pointer<Void> session,
  int shotIndex,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint64)>(
  symbol: 'kineto_project_shot_resubmit_keyframes',
)
external int _kinetoProjectShotResubmitKeyframes(
  Pointer<Void> session,
  int shotIndex,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Uint64)>(
  symbol: 'kineto_project_shot_select',
)
external int _kinetoProjectShotSelect(
  Pointer<Void> session,
  int shotIndex,
  int candidateIndex,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint64)>(
  symbol: 'kineto_project_shot_lock',
)
external int _kinetoProjectShotLock(
  Pointer<Void> session,
  int shotIndex,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint64)>(
  symbol: 'kineto_project_shot_reset',
)
external int _kinetoProjectShotReset(
  Pointer<Void> session,
  int shotIndex,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint64)>(
  symbol: 'kineto_project_shot_migrate_legacy_hashes',
)
external int _kinetoProjectShotMigrateLegacyHashes(
  Pointer<Void> session,
  int shotIndex,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint64)>(
  symbol: 'kineto_project_shot_generate_video',
)
external int _kinetoProjectShotGenerateVideo(
  Pointer<Void> session,
  int shotIndex,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint64)>(
  symbol: 'kineto_project_shot_resubmit_video',
)
external int _kinetoProjectShotResubmitVideo(
  Pointer<Void> session,
  int shotIndex,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint64)>(
  symbol: 'kineto_project_shot_lock_video',
)
external int _kinetoProjectShotLockVideo(
  Pointer<Void> session,
  int shotIndex,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Pointer<Uint8>, Uint64, Pointer<Uint64>)>(
  symbol: 'kineto_project_video_media_copy',
)
external int _kinetoProjectVideoMediaCopy(
  Pointer<Void> session,
  int shotIndex,
  Pointer<Uint8> outBuffer,
  int outCapacity,
  Pointer<Uint64> outLength,
);

@Native<Uint32 Function(Pointer<Void>)>(
  symbol: 'kineto_project_image_provider',
  isLeaf: true,
)
external int _kinetoProjectImageProvider(Pointer<Void> session);

@Native<Uint8 Function(Pointer<Void>)>(
  symbol: 'kineto_project_image_provider_ready',
  isLeaf: true,
)
external int _kinetoProjectImageProviderReady(Pointer<Void> session);

@Native<Uint64 Function(Pointer<Void>)>(
  symbol: 'kineto_project_generation_cost_micros',
  isLeaf: true,
)
external int _kinetoProjectGenerationCostMicros(Pointer<Void> session);

@Native<Uint64 Function(Pointer<Void>)>(
  symbol: 'kineto_project_video_cost_micros',
  isLeaf: true,
)
external int _kinetoProjectVideoCostMicros(Pointer<Void> session);

@Native<
    Int32 Function(
      Pointer<Void>,
      Uint32,
      Uint32,
      Pointer<Uint8>,
      Uint64,
      Pointer<Uint64>,
    )>(symbol: 'kineto_project_candidate_media_copy')
external int _kinetoProjectCandidateMediaCopy(
  Pointer<Void> session,
  int shotIndex,
  int candidateIndex,
  Pointer<Uint8> outBuffer,
  int outCapacity,
  Pointer<Uint64> outLength,
);

@Native<
    Int32 Function(
      Pointer<Void>,
      Uint32,
      Uint32,
      Pointer<Uint8>,
      Uint64,
      Pointer<Uint64>,
    )>(symbol: 'kineto_project_candidate_provider_copy')
external int _kinetoProjectCandidateProviderCopy(
  Pointer<Void> session,
  int shotIndex,
  int candidateIndex,
  Pointer<Uint8> outBuffer,
  int outCapacity,
  Pointer<Uint64> outLength,
);

@Native<
    Int32 Function(
      Pointer<Void>,
      Uint32,
      Uint32,
      Pointer<Uint8>,
      Uint64,
      Pointer<Uint64>,
    )>(symbol: 'kineto_project_candidate_model_copy')
external int _kinetoProjectCandidateModelCopy(
  Pointer<Void> session,
  int shotIndex,
  int candidateIndex,
  Pointer<Uint8> outBuffer,
  int outCapacity,
  Pointer<Uint64> outLength,
);

/// Who generates a session's shot candidates. Chosen when Kineto starts, by
/// `KINETO_IMAGE_PROVIDER`; a paid provider is never chosen implicitly.
enum KinetoImageProviderKind {
  localDemo(1, 'local_demo', 'Local demo'),
  minimax(2, 'minimax', 'MiniMax image-01');

  const KinetoImageProviderKind(this.wireValue, this.wireName, this.label);

  final int wireValue;
  final String wireName;
  final String label;

  static KinetoImageProviderKind fromWireValue(int value) {
    for (final kind in values) {
      if (kind.wireValue == value) return kind;
    }
    throw StateError('Unknown Kineto image provider code $value');
  }
}

/// Who made a candidate: the provider and model recorded on its artifact.
/// Not the session's provider, which may have changed since.
final class KinetoCandidateOrigin {
  const KinetoCandidateOrigin({required this.provider, required this.model});

  /// The recorded provider, for example `minimax`; empty when none is recorded.
  final String provider;

  /// The recorded model, for example `image-01`; empty when none is recorded.
  final String model;

  static const Map<String, String> _providerNames = <String, String>{
    'local-demo': 'Local demo',
    'minimax': 'MiniMax',
  };

  /// For example `MiniMax image-01`; null when no provider is recorded.
  String? get label {
    if (provider.isEmpty) return null;
    final name = _providerNames[provider] ?? provider;
    return model.isEmpty ? name : '$name $model';
  }
}

/// The session's image provider, as the director needs to know it before
/// asking for a generation.
final class KinetoImageProvider {
  const KinetoImageProvider({
    required this.kind,
    required this.ready,
    required this.generationCostMicros,
    this.videoCostMicros = 0,
  });

  final KinetoImageProviderKind kind;

  /// False for a paid provider without credentials: generating would fail.
  final bool ready;

  /// Estimated cost of one generation (every candidate), in USD micros.
  final int generationCostMicros;

  /// Estimated cost of one shot video, in USD micros; 0 when this provider
  /// cannot generate video.
  final int videoCostMicros;

  bool get isPaid => kind != KinetoImageProviderKind.localDemo;

  /// Whether this session can generate a shot video at all.
  bool get canGenerateVideo => ready && videoCostMicros > 0;

  /// The generation estimate in US dollars, for example `$0.0105`.
  String get generationCostLabel => _dollars(generationCostMicros);

  /// The video estimate in US dollars, for example `$0.28`.
  String get videoCostLabel => _dollars(videoCostMicros);

  static String _dollars(int amountMicros) {
    final dollars = amountMicros ~/ 1000000;
    final micros = (amountMicros % 1000000).toString().padLeft(6, '0');
    final trimmed = micros.replaceFirst(RegExp(r'0+$'), '');
    return trimmed.isEmpty ? '\$$dollars' : '\$$dollars.$trimmed';
  }
}

final class KinetoProjectException implements Exception {
  const KinetoProjectException(this.code, this.message);

  /// The create target already exists; nothing was overwritten.
  static const int alreadyExists = 103;

  /// Another live session holds the project's single-writer lock.
  static const int busy = 117;

  /// The shot's stored hashes need explicit migration before it can change.
  static const int migrationRequired = 118;

  /// The single-writer lock cannot be provided here (for example the
  /// filesystem cannot lock files), so the project is not opened for writing.
  static const int lockUnavailable = 119;

  /// A generation job stopped before it finished. Its write-ahead intents are
  /// kept and the generation is completed when the project is next opened.
  static const int generationJob = 121;

  /// The selected image provider has no credentials configured.
  static const int providerNotConfigured = 123;

  /// The shot has no written intent for a real provider to generate from.
  static const int shotIntentMissing = 124;

  /// The image provider refused the request; asking again unchanged will not
  /// help (for example a content or account refusal).
  static const int providerRefused = 125;

  /// The shot's video is still being generated.
  static const int videoInFlight = 127;

  /// A screenplay is written from a story analysis, which does not exist yet.
  static const int needsAnalysis = 133;

  /// A newer screenplay revision exists than the one the edit started from.
  static const int notLatest = 136;

  /// No language model is configured for documents.
  static const int modelNotConfigured = 140;

  /// The language model's cost is not known, as for an agent on a
  /// subscription.
  static const int costUnknown = 143;

  final int code;
  final String message;

  @override
  String toString() => 'KinetoProjectException($code): $message';
}

/// Thin Dart owner for an opened canonical Kineto project.
///
/// The project runs on its own Rust worker thread. Every operation that may
/// touch the filesystem returns a [Future] and never blocks this isolate; the
/// worker reports back through a port owned by this session. Snapshot and
/// metadata reads return state the worker has already published and never do
/// I/O. Strings cross the native boundary only as borrowed UTF-8
/// pointer/length pairs; TOML, JSON, and serialized project models do not.
///
/// A session belongs to the isolate that opened it: its port delivers there.
/// Call [close] when done; it waits for at most the running command.
final class KinetoProjectSession implements Finalizable {
  KinetoProjectSession._(this._handle, this._port) {
    _finalizer.attach(this, _handle, detach: this);
    _portFinalizer.attach(this, _port, detach: this);
  }

  static const int shotCount = 2;

  static final NativeFinalizer _finalizer = NativeFinalizer(
    Native.addressOf<NativeFunction<_DestroyProjectNative>>(
      _kinetoProjectDestroy,
    ),
  );

  // A session collected without close() also releases its port. It can only
  // be collected while nothing is owed: the port holds the session until
  // every answer has arrived.
  static final Finalizer<_SessionPort> _portFinalizer =
      Finalizer<_SessionPort>((port) => port.close());

  final Pointer<Void> _handle;
  final _SessionPort _port;
  bool _closed = false;

  /// Create a canonical text-source project at [path] and open it.
  static Future<KinetoProjectSession> createTextProject({
    required String path,
    required String projectId,
    required String title,
    required String createdAt,
    required String language,
    required String sourceText,
  }) {
    _assertAbiCompatible();
    final port = _SessionPort();
    final arena = Arena();
    try {
      final pathBytes = _encodeUtf8(arena, path);
      final idBytes = _encodeUtf8(arena, projectId);
      final titleBytes = _encodeUtf8(arena, title);
      final createdAtBytes = _encodeUtf8(arena, createdAt);
      final languageBytes = _encodeUtf8(arena, language);
      final sourceBytes = _encodeUtf8(arena, sourceText);
      final out = arena<Pointer<Void>>();
      out.value = nullptr;

      final code = _kinetoProjectCreateText(
        pathBytes.pointer,
        pathBytes.length,
        idBytes.pointer,
        idBytes.length,
        titleBytes.pointer,
        titleBytes.length,
        createdAtBytes.pointer,
        createdAtBytes.length,
        languageBytes.pointer,
        languageBytes.length,
        sourceBytes.pointer,
        sourceBytes.length,
        NativeApi.postCObject,
        port.nativePort,
        out,
      );
      return _finishOpen(code, out.value, port);
    } finally {
      arena.releaseAll();
    }
  }

  /// Open the canonical project at [path].
  static Future<KinetoProjectSession> open(String path) {
    _assertAbiCompatible();
    final port = _SessionPort();
    final arena = Arena();
    try {
      final pathBytes = _encodeUtf8(arena, path);
      final out = arena<Pointer<Void>>();
      out.value = nullptr;
      final code = _kinetoProjectOpen(
        pathBytes.pointer,
        pathBytes.length,
        NativeApi.postCObject,
        port.nativePort,
        out,
      );
      return _finishOpen(code, out.value, port);
    } finally {
      arena.releaseAll();
    }
  }

  static Future<KinetoProjectSession> _finishOpen(
    int code,
    Pointer<Void> handle,
    _SessionPort port,
  ) async {
    if (code != 0) {
      port.close();
      throw _projectException(code);
    }
    if (handle == nullptr) {
      port.close();
      throw StateError('Kineto native project open returned a null session');
    }
    final session = KinetoProjectSession._(handle, port);
    try {
      await port.opened;
    } on Object {
      session.close();
      rethrow;
    }
    return session;
  }

  /// Work an earlier session left unfinished and this open dealt with, in
  /// the order the engine reported it. Known once the open has completed.
  List<KinetoRecovery> get recoveries => List.unmodifiable(_port.recoveries);

  int get formatVersion {
    _ensureOpen();
    return _kinetoProjectFormatVersion(_handle);
  }

  bool get isReadOnly {
    _ensureOpen();
    return _kinetoProjectIsReadOnly(_handle) != 0;
  }

  String get projectId {
    _ensureOpen();
    return _copyProjectUtf8(_handle, _kinetoProjectIdCopy, 'project id');
  }

  String get title {
    _ensureOpen();
    return _copyProjectUtf8(_handle, _kinetoProjectTitleCopy, 'project title');
  }

  /// Absolute path of the media file of the shot's candidate
  /// [candidateIndex], or null for a candidate without media (a demo
  /// candidate). Reads published state; never does I/O.
  String? candidateMediaPath(int shotIndex, int candidateIndex) {
    _ensureOpen();
    _checkShotIndex(shotIndex);
    if (candidateIndex < 0) {
      throw RangeError.value(
        candidateIndex,
        'candidateIndex',
        'Candidate index must be non-negative',
      );
    }
    final path = _copyProjectUtf8(
      _handle,
      (session, buffer, capacity, length) => _kinetoProjectCandidateMediaCopy(
        session,
        shotIndex,
        candidateIndex,
        buffer,
        capacity,
        length,
      ),
      'candidate media path',
    );
    return path.isEmpty ? null : path;
  }

  /// Absolute path of the shot's current video, or null when it has none.
  /// Reads published state; never does I/O.
  String? videoMediaPath(int shotIndex) {
    _ensureOpen();
    _checkShotIndex(shotIndex);
    final path = _copyProjectUtf8(
      _handle,
      (session, buffer, capacity, length) =>
          _kinetoProjectVideoMediaCopy(session, shotIndex, buffer, capacity, length),
      'video media path',
    );
    return path.isEmpty ? null : path;
  }

  /// Videos generated in the background as they finish: attached, or failed
  /// and ready to be asked for again.
  Stream<KinetoVideoEvent> get videoEvents => _port.videoEvents;

  /// Submit a video from the shot's locked keyframe. Completes once MiniMax
  /// has accepted the task; the video arrives on [videoEvents].
  Future<void> generateShotVideo(int shotIndex) => _submit(
        shotIndex,
        (token) => _kinetoProjectShotGenerateVideo(_handle, shotIndex, token),
      );

  /// Submit the shot's video again although its last submission is
  /// unresolved: MiniMax may already have accepted, and charged for, it.
  /// Only on the director's explicit decision to risk paying twice.
  Future<void> resubmitShotVideo(int shotIndex) => _submit(
        shotIndex,
        (token) => _kinetoProjectShotResubmitVideo(_handle, shotIndex, token),
      );

  Future<void> lockShotVideo(int shotIndex) => _submit(
        shotIndex,
        (token) => _kinetoProjectShotLockVideo(_handle, shotIndex, token),
      );

  /// The provider and model recorded on the shot's candidate
  /// [candidateIndex]. Reads published state; never does I/O.
  KinetoCandidateOrigin candidateOrigin(int shotIndex, int candidateIndex) {
    _ensureOpen();
    _checkShotIndex(shotIndex);
    if (candidateIndex < 0) {
      throw RangeError.value(
        candidateIndex,
        'candidateIndex',
        'Candidate index must be non-negative',
      );
    }
    return KinetoCandidateOrigin(
      provider: _copyProjectUtf8(
        _handle,
        (session, buffer, capacity, length) => _kinetoProjectCandidateProviderCopy(
          session,
          shotIndex,
          candidateIndex,
          buffer,
          capacity,
          length,
        ),
        'candidate provider',
      ),
      model: _copyProjectUtf8(
        _handle,
        (session, buffer, capacity, length) => _kinetoProjectCandidateModelCopy(
          session,
          shotIndex,
          candidateIndex,
          buffer,
          capacity,
          length,
        ),
        'candidate model',
      ),
    );
  }

  /// The image provider this session generates with. Fixed when it opened.
  KinetoImageProvider get imageProvider {
    _ensureOpen();
    return KinetoImageProvider(
      kind: KinetoImageProviderKind.fromWireValue(
        _kinetoProjectImageProvider(_handle),
      ),
      ready: _kinetoProjectImageProviderReady(_handle) != 0,
      generationCostMicros: _kinetoProjectGenerationCostMicros(_handle),
      videoCostMicros: _kinetoProjectVideoCostMicros(_handle),
    );
  }

  /// The shot's most recently published state. Never does I/O.
  KinetoShotSnapshot shotSnapshot(int shotIndex) {
    _ensureOpen();
    _checkShotIndex(shotIndex);
    final arena = Arena();
    try {
      final outState = arena<Uint64>();
      outState.value = 0;
      _checkProjectResult(
        _kinetoProjectShotState(_handle, shotIndex, outState),
      );
      return KinetoShotSnapshot.fromBits(outState.value);
    } finally {
      arena.releaseAll();
    }
  }

  /// Whether a paid keyframe call of the shot's last generation may have
  /// been accepted, and charged, without leaving its result. Generating is
  /// refused until [resubmitShotKeyframes]. Reads published state; never
  /// does I/O.
  bool keyframesUnresolved(int shotIndex) {
    _ensureOpen();
    _checkShotIndex(shotIndex);
    final arena = Arena();
    try {
      final outUnresolved = arena<Uint8>();
      outUnresolved.value = 0;
      _checkProjectResult(
        _kinetoProjectShotKeyframesUnresolved(_handle, shotIndex, outUnresolved),
      );
      return outUnresolved.value != 0;
    } finally {
      arena.releaseAll();
    }
  }

  Future<void> setShotDirection(int shotIndex, KinetoShotDirection direction) =>
      _submit(
        shotIndex,
        (token) => _kinetoProjectShotSetDirection(
          _handle,
          shotIndex,
          direction.wireValue,
          token,
        ),
      );

  Future<void> generateShotCandidates(int shotIndex) => _submit(
        shotIndex,
        (token) => _kinetoProjectShotGenerate(_handle, shotIndex, token),
      );

  /// Generate the shot's candidates again although a paid call of the last
  /// attempt may already have been accepted, and charged for. Only on the
  /// director's explicit decision to risk paying twice.
  Future<void> resubmitShotKeyframes(int shotIndex) => _submit(
        shotIndex,
        (token) => _kinetoProjectShotResubmitKeyframes(_handle, shotIndex, token),
      );

  Future<void> selectShotCandidate(int shotIndex, int candidateIndex) {
    if (candidateIndex < 0) {
      throw RangeError.value(
        candidateIndex,
        'candidateIndex',
        'Candidate index must be non-negative',
      );
    }
    return _submit(
      shotIndex,
      (token) =>
          _kinetoProjectShotSelect(_handle, shotIndex, candidateIndex, token),
    );
  }

  Future<void> lockShotSelection(int shotIndex) => _submit(
        shotIndex,
        (token) => _kinetoProjectShotLock(_handle, shotIndex, token),
      );

  Future<void> resetShot(int shotIndex) => _submit(
        shotIndex,
        (token) => _kinetoProjectShotReset(_handle, shotIndex, token),
      );

  /// Rewrite this shot's known legacy hashes to canonical form, after writing
  /// a backup of the original records. Opening a project never does this.
  Future<void> migrateLegacyHashes(int shotIndex) => _submit(
        shotIndex,
        (token) =>
            _kinetoProjectShotMigrateLegacyHashes(_handle, shotIndex, token),
      );

  /// Close the session. Commands that have not started are discarded and their
  /// futures fail; the command already running finishes before this returns,
  /// and the project's single-writer lock is released.
  void close() {
    if (_closed) return;
    _closed = true;
    _finalizer.detach(this);
    _portFinalizer.detach(this);
    // Destroy first: after it returns the worker posts nothing more, so the
    // port can be closed without losing a message that still matters.
    _kinetoProjectDestroy(_handle);
    _port.close();
  }

  Future<void> _submit(int shotIndex, int Function(int token) queue) {
    _checkShotIndex(shotIndex);
    return _submitDocument(queue);
  }

  /// Queue a command that answers with a `COMPLETED` message for [queue]'s
  /// token, whatever it concerns.
  Future<void> _submitDocument(int Function(int token) queue) {
    _ensureOpen();
    final (token, done) = _port.begin(this);
    final code = queue(token);
    if (code != 0) _port.fail(token, _projectException(code));
    return done;
  }

  void _checkShotIndex(int shotIndex) {
    if (shotIndex < 0 || shotIndex >= shotCount) {
      throw RangeError.range(shotIndex, 0, shotCount - 1, 'shotIndex');
    }
  }

  void _ensureOpen() {
    if (_closed) {
      throw StateError('Kineto project session is closed');
    }
  }
}

/// Receives one session's `[kind, token, status, shot]` messages from the
/// native worker and completes the matching futures.
final class _SessionPort {
  _SessionPort() {
    _raw = RawReceivePort(_onMessage, 'kineto project session');
    _updateKeepAlive();
  }

  static const int _opened = 1;
  static const int _completed = 2;
  static const int _recovered = 3;
  static const int _video = 4;

  final StreamController<KinetoVideoEvent> _videoEvents =
      StreamController<KinetoVideoEvent>.broadcast(sync: false);

  Stream<KinetoVideoEvent> get videoEvents => _videoEvents.stream;

  late final RawReceivePort _raw;
  final Completer<void> _openCompleter = Completer<void>();

  // While an answer is owed, the port holds its session, so garbage
  // collection cannot finalize the session and suppress that answer. The
  // field exists only to keep the session reachable; it is never read.
  // ignore: unused_field
  Object? _owner;
  final Map<int, Completer<void>> _pending = <int, Completer<void>>{};
  final List<KinetoRecovery> recoveries = <KinetoRecovery>[];
  int _nextToken = 1;
  bool _closed = false;

  int get nativePort => _raw.sendPort.nativePort;

  Future<void> get opened => _openCompleter.future;

  (int, Future<void>) begin(Object owner) {
    final token = _nextToken++;
    final completer = Completer<void>();
    _pending[token] = completer;
    _owner = owner;
    _updateKeepAlive();
    return (token, completer.future);
  }

  void fail(int token, Object error) {
    _pending.remove(token)?.completeError(error);
    _updateKeepAlive();
  }

  void close() {
    if (_closed) return;
    _closed = true;
    _owner = null;
    _raw.close();
    unawaited(_videoEvents.close());
    const error = KinetoProjectSessionClosedException();
    if (!_openCompleter.isCompleted) _openCompleter.completeError(error);
    for (final completer in _pending.values) {
      completer.completeError(error);
    }
    _pending.clear();
  }

  void _onMessage(Object? message) {
    final values = message! as List<int>;
    final status = values[2];
    switch (values[0]) {
      case _opened:
        if (_openCompleter.isCompleted) break;
        if (status == 0) {
          _openCompleter.complete();
        } else {
          _openCompleter.completeError(_projectException(status));
        }
      case _recovered:
        // Posted before OPENED, so recoveries are complete when open returns.
        recoveries.add(
          KinetoRecovery(
            shotIndex: values[3] < 0 ? null : values[3],
            revisionOrCount: values[1],
            errorCode: status,
            document: switch (values[3]) {
              -2 => KinetoDocumentKind.storyAnalysis,
              -3 => KinetoDocumentKind.screenplay,
              _ => null,
            },
          ),
        );
      case _video:
        _videoEvents.add(
          KinetoVideoEvent(
            shotIndex: values[3],
            revision: values[1],
            errorCode: status,
          ),
        );
      case _completed:
        final completer = _pending.remove(values[1]);
        if (completer == null) break;
        if (status == 0) {
          completer.complete();
        } else {
          completer.completeError(_projectException(status));
        }
    }
    _updateKeepAlive();
  }

  // Keep the isolate alive only while an answer is still owed, so a session
  // that is merely open never pins its isolate.
  void _updateKeepAlive() {
    if (_closed) return;
    _raw.keepIsolateAlive = !_openCompleter.isCompleted || _pending.isNotEmpty;
    if (_pending.isEmpty) _owner = null;
  }
}

/// A video generated in the background finished.
final class KinetoVideoEvent {
  const KinetoVideoEvent({
    required this.shotIndex,
    required this.revision,
    required this.errorCode,
  });

  final int shotIndex;
  final int revision;

  /// 0 when the video was attached; otherwise why it was not.
  final int errorCode;

  KinetoProjectException? get error =>
      errorCode == 0 ? null : _projectException(errorCode);
}

/// Unfinished work from an earlier session that an open dealt with.
final class KinetoRecovery {
  const KinetoRecovery({
    required this.shotIndex,
    required this.revisionOrCount,
    required this.errorCode,
    this.document,
  });

  /// The shot whose interrupted generation this was; null for a document and
  /// for job records the engine could not read or place.
  final int? shotIndex;

  /// The document a model answered before the interruption, kept as
  /// revision [revisionOrCount] when the project opened.
  final KinetoDocumentKind? document;

  /// The generation or document revision, or the number of unplaced records.
  final int revisionOrCount;

  /// 0 when the generation was completed; otherwise why it was not.
  final int errorCode;

  /// The interrupted generation was completed when the project opened.
  bool get completed => (shotIndex != null || document != null) && errorCode == 0;

  /// Job records that could not be read or placed; nothing was changed.
  bool get needsAttention => shotIndex == null && document == null;

  /// Why the recovery did not complete, when it did not.
  KinetoProjectException? get error =>
      errorCode == 0 ? null : _projectException(errorCode);
}

/// A queued operation was abandoned because its session was closed first.
final class KinetoProjectSessionClosedException implements Exception {
  const KinetoProjectSessionClosedException();

  @override
  String toString() =>
      'KinetoProjectSessionClosedException: the project session was closed '
      'before the operation ran';
}

final class _NativeBytes {
  const _NativeBytes(this.pointer, this.length);

  final Pointer<Uint8> pointer;
  final int length;
}

_NativeBytes _encodeUtf8(Arena arena, String value) {
  final bytes = utf8.encode(value);
  final pointer = arena<Uint8>(bytes.isEmpty ? 1 : bytes.length);
  if (bytes.isNotEmpty) {
    pointer.asTypedList(bytes.length).setAll(0, bytes);
  }
  return _NativeBytes(pointer, bytes.length);
}

String _copyProjectUtf8(
  Pointer<Void> session,
  _ProjectTextCopyDart copy,
  String field,
) {
  final arena = Arena();
  try {
    final outLength = arena<Uint64>();
    outLength.value = 0;
    final sizingCode = copy(session, nullptr, 0, outLength);
    if (sizingCode == 0 && outLength.value == 0) return '';
    if (sizingCode != 106) _checkProjectResult(sizingCode);

    final capacity = outLength.value;
    if (capacity == 0) return '';
    final buffer = arena<Uint8>(capacity);
    outLength.value = 0;
    final copyCode = copy(session, buffer, capacity, outLength);
    if (copyCode == 106) {
      throw StateError('Kineto native $field changed while it was being copied');
    }
    _checkProjectResult(copyCode);
    if (outLength.value > capacity) {
      throw StateError('Kineto native $field length exceeded the allocated buffer');
    }
    return utf8.decode(
      buffer.asTypedList(outLength.value),
      allowMalformed: false,
    );
  } finally {
    arena.releaseAll();
  }
}

void _checkProjectResult(int code) {
  if (code == 0) return;
  throw _projectException(code);
}

/// Project status codes by the names the native contract uses. Tests check
/// this table, and the named constants on [KinetoProjectException], against
/// the bundled library.
const Map<String, int> kinetoProjectStatusCodes = <String, int>{
  'ok': 0,
  'invalid_argument': 100,
  'invalid_utf8': 101,
  'io': 102,
  'already_exists': 103,
  'invalid_manifest': 104,
  'unsupported_format': 105,
  'buffer_too_small': 106,
  'read_only': 107,
  'invalid_shot': 108,
  'not_generated': 109,
  'invalid_candidate': 110,
  'locked': 111,
  'already_locked': 112,
  'no_selection': 113,
  'stale_selection': 114,
  'invalid_shot_state': 115,
  'generation_overflow': 116,
  'busy': 117,
  'migration_required': 118,
  'lock_unavailable': 119,
  'not_open': 120,
  'generation_job': 121,
  'provider_unknown': 122,
  'provider_not_configured': 123,
  'shot_intent_missing': 124,
  'provider_refused': 125,
  'paid_call_unresolved': 126,
  'video_in_flight': 127,
  'keyframe_not_locked': 128,
  'video_locked': 129,
  'no_video': 130,
  'stale_video': 131,
  'no_first_frame': 132,
  'needs_analysis': 133,
  'needs_screenplay': 134,
  'no_such_scene': 135,
  'not_latest': 136,
  'invalid_document': 137,
  'invalid_revision': 138,
  'invalid_source': 139,
  'model_not_configured': 140,
  'model_unknown': 141,
  'document_damaged': 142,
  'cost_unknown': 143,
  'panic': 199,
};

/// What each non-zero project status means to the director.
const Map<int, String> kinetoProjectStatusMessages = <int, String>{
  100: 'Invalid project argument',
  101: 'Project input is not valid UTF-8',
  102: 'Project filesystem operation failed',
  103: 'Project target already exists',
  104: 'Project manifest is invalid',
  105: 'Project format is newer than this Kineto build supports',
  106: 'Project metadata buffer is too small',
  107: 'Project is read-only until an explicit migration is performed',
  108: 'Shot index is invalid',
  109: 'Generate candidates before selecting',
  110: 'Candidate index is invalid',
  111: 'Shot selection is locked',
  112: 'Shot selection is already locked',
  113: 'Select a candidate before locking',
  114: 'Selection is stale; regenerate before locking',
  115: 'Canonical shot approval state is invalid',
  116: 'Shot generation revision overflow',
  117: 'Project is already open for writing in another Kineto window',
  118: 'Legacy shot hashes need migration before this shot can change',
  119: 'Project cannot be locked for single-writer access on this filesystem',
  120: 'Project session is not open',
  121: 'Generation did not finish. Finished candidates are kept; generate again to retry the rest',
  122: 'KINETO_IMAGE_PROVIDER names an image provider this Kineto build does not have',
  123: 'The image provider has no API key configured',
  124: 'This shot has no written intent to generate from',
  125: 'The provider refused the request; it will not succeed unchanged',
  126: 'A paid call may already have been accepted and charged without its result being kept; '
      'asking again may pay twice',
  127: "This shot's video is still being generated",
  128: 'Lock a keyframe before generating video',
  129: "This shot's video is locked",
  130: 'Generate a video before locking it',
  131: 'The video was made from a keyframe that is no longer locked; regenerate it',
  132: 'The locked keyframe has no image to start a video from',
  133: 'Analyse the story before writing the screenplay',
  134: 'Write the screenplay before rewriting a scene',
  135: 'The screenplay has no scene with that id',
  136: 'A newer screenplay revision exists; review it before saving',
  137: 'The edit breaks the screenplay: every line of dialogue needs a cast member, and no block may be empty',
  138: 'Only the latest screenplay revision can be locked',
  139: 'The source must be non-empty text of at most 8 MiB',
  140: 'No language model is configured; set KINETO_LLM_PROVIDER',
  141: 'KINETO_LLM_PROVIDER names a language model this Kineto build does not have',
  142: 'A stored document no longer matches its record',
  143: "This model's cost is not known",
  199: 'Native project call failed unexpectedly',
};

KinetoProjectException _projectException(int code) => KinetoProjectException(
      code,
      kinetoProjectStatusMessages[code] ?? 'Unknown native project error',
    );
