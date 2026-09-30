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
    _ensureOpen();
    _checkShotIndex(shotIndex);
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

  late final RawReceivePort _raw;
  final Completer<void> _openCompleter = Completer<void>();

  // While an answer is owed, the port holds its session, so garbage
  // collection cannot finalize the session and suppress that answer. The
  // field exists only to keep the session reachable; it is never read.
  // ignore: unused_field
  Object? _owner;
  final Map<int, Completer<void>> _pending = <int, Completer<void>>{};
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

KinetoProjectException _projectException(int code) {
  return KinetoProjectException(code, switch (code) {
    100 => 'Invalid project argument',
    101 => 'Project input is not valid UTF-8',
    102 => 'Project filesystem operation failed',
    103 => 'Project target already exists',
    104 => 'Project manifest is invalid',
    105 => 'Project format is newer than this Kineto build supports',
    106 => 'Project metadata buffer is too small',
    107 => 'Project is read-only until an explicit migration is performed',
    108 => 'Shot index is invalid',
    109 => 'Generate candidates before selecting',
    110 => 'Candidate index is invalid',
    111 => 'Shot selection is locked',
    112 => 'Shot selection is already locked',
    113 => 'Select a candidate before locking',
    114 => 'Selection is stale; regenerate before locking',
    115 => 'Canonical shot approval state is invalid',
    116 => 'Shot generation revision overflow',
    117 => 'Project is already open for writing in another Kineto window',
    118 => 'Legacy shot hashes need migration before this shot can change',
    119 => 'Project cannot be locked for single-writer access on this filesystem',
    120 => 'Project session is not open',
    199 => 'Native project call failed unexpectedly',
    _ => 'Unknown native project error',
  });
}
