part of 'kineto_engine.dart';

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Pointer<Uint8>, Uint64, Pointer<Uint8>, Uint64, Bool, Uint64)>(
  symbol: 'kineto_project_shot_generate_videos',
)
external int _kinetoProjectShotGenerateVideos(
  Pointer<Void> session,
  int shotIndex,
  int count,
  Pointer<Uint8> note,
  int noteLength,
  Pointer<Uint8> branch,
  int branchLength,
  bool resubmit,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Uint64)>(symbol: 'kineto_project_shot_select_video')
external int _kinetoProjectShotSelectVideo(Pointer<Void> session, int shotIndex, int video, int token);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Uint32, Pointer<Uint32>)>(
  symbol: 'kineto_project_video_take_count',
  isLeaf: true,
)
external int _kinetoProjectVideoTakeCount(Pointer<Void> session, int what, int shotIndex, int take, Pointer<Uint32> out);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Uint32, Pointer<Uint64>)>(
  symbol: 'kineto_project_video_take_value',
  isLeaf: true,
)
external int _kinetoProjectVideoTakeValue(Pointer<Void> session, int field, int shotIndex, int take, Pointer<Uint64> out);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Uint32, Uint32, Pointer<Uint8>, Uint64, Pointer<Uint64>)>(
  symbol: 'kineto_project_video_take_text_copy',
  isLeaf: true,
)
external int _kinetoProjectVideoTakeTextCopy(
  Pointer<Void> session,
  int field,
  int shotIndex,
  int take,
  int item,
  Pointer<Uint8> buffer,
  int capacity,
  Pointer<Uint64> length,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Pointer<Uint32>)>(symbol: 'kineto_project_progress_count', isLeaf: true)
external int _kinetoProjectProgressCount(Pointer<Void> session, int scene, Pointer<Uint32> out);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Uint32, Pointer<Uint64>)>(
  symbol: 'kineto_project_progress_value',
  isLeaf: true,
)
external int _kinetoProjectProgressValue(Pointer<Void> session, int field, int scene, int shot, Pointer<Uint64> out);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Uint32, Pointer<Uint8>, Uint64, Pointer<Uint64>)>(
  symbol: 'kineto_project_progress_text_copy',
  isLeaf: true,
)
external int _kinetoProjectProgressTextCopy(
  Pointer<Void> session,
  int field,
  int scene,
  int shot,
  Pointer<Uint8> buffer,
  int capacity,
  Pointer<Uint64> length,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint64)>(symbol: 'kineto_project_storage')
external int _kinetoProjectStorage(Pointer<Void> session, int command, int token);

@Native<Int32 Function(Pointer<Void>, Uint32, Pointer<Uint64>)>(symbol: 'kineto_project_media_value', isLeaf: true)
external int _kinetoProjectMediaValue(Pointer<Void> session, int field, Pointer<Uint64> out);

@Native<Int32 Function(Pointer<Void>, Uint32, Pointer<Uint32>)>(symbol: 'kineto_project_media_count', isLeaf: true)
external int _kinetoProjectMediaCount(Pointer<Void> session, int list, Pointer<Uint32> out);

@Native<Int32 Function(Pointer<Void>, Uint32, Pointer<Uint64>)>(
  symbol: 'kineto_project_media_owner_bytes',
  isLeaf: true,
)
external int _kinetoProjectMediaOwnerBytes(Pointer<Void> session, int index, Pointer<Uint64> out);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Uint32, Pointer<Uint8>, Uint64, Pointer<Uint64>)>(
  symbol: 'kineto_project_media_text_copy',
  isLeaf: true,
)
external int _kinetoProjectMediaTextCopy(
  Pointer<Void> session,
  int list,
  int index,
  int field,
  Pointer<Uint8> buffer,
  int capacity,
  Pointer<Uint64> length,
);

@Native<Int32 Function(Pointer<Void>, Pointer<Uint8>, Uint64, Pointer<Uint8>, Uint64, Bool, Bool, Uint64)>(
  symbol: 'kineto_project_export',
)
external int _kinetoProjectExport(
  Pointer<Void> session,
  Pointer<Uint8> destination,
  int destinationLength,
  Pointer<Uint8> name,
  int nameLength,
  bool partial,
  bool replace,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Pointer<Uint64>)>(symbol: 'kineto_project_export_value', isLeaf: true)
external int _kinetoProjectExportValue(Pointer<Void> session, int field, Pointer<Uint64> out);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Pointer<Uint8>, Uint64, Pointer<Uint64>)>(
  symbol: 'kineto_project_export_text_copy',
  isLeaf: true,
)
external int _kinetoProjectExportTextCopy(
  Pointer<Void> session,
  int field,
  int index,
  Pointer<Uint8> buffer,
  int capacity,
  Pointer<Uint64> length,
);

/// Most video candidates one generation may be asked for.
const int kinetoMaxVideoCandidates = 4;

/// One video a shot has had.
final class KinetoVideoTake {
  const KinetoVideoTake({
    required this.id,
    required this.generation,
    required this.status,
    this.candidate,
    this.mediaPath,
    this.provider = '',
    this.model = '',
    this.notes = const <String>[],
    this.branchedFrom,
    this.stale = const <String>[],
  });

  final String id;
  final int generation;

  /// Its place among its generation's candidates, from 0; null for a video
  /// made before a generation could have several.
  final int? candidate;
  final KinetoTakeStatus status;

  /// Absolute path of the clip; null once its bytes were let go.
  final String? mediaPath;
  final String provider;
  final String model;

  /// The director's notes it was made with, oldest first.
  final List<String> notes;

  /// The take it was branched from.
  final String? branchedFrom;

  /// Why it no longer follows the shot; empty when it does.
  final List<String> stale;

  /// A take of the latest generation, not superseded.
  bool get current => status != KinetoTakeStatus.superseded;

  /// `b`, for the second candidate; empty for an old single video.
  String get letter => candidate == null ? '' : String.fromCharCode(0x61 + candidate!);
}

/// How far one shot's keyframe or video has come.
enum KinetoStage {
  missing('missing'),
  made('made'),
  locked('locked'),
  stale('stale');

  const KinetoStage(this.wireName);

  final String wireName;
}

final class KinetoShotProgress {
  const KinetoShotProgress({required this.shotId, required this.keyframe, required this.video});

  final String shotId;
  final KinetoStage keyframe;
  final KinetoStage video;
}

/// The shots in play of one scene, and whether every one has its video
/// locked.
final class KinetoSceneProgress {
  const KinetoSceneProgress({required this.sceneId, required this.shots, required this.complete});

  final String sceneId;
  final List<KinetoShotProgress> shots;
  final bool complete;
}

/// A media file a list names: its artifact, its owner and, in a dry run,
/// why it would be let go.
final class KinetoMediaItem {
  const KinetoMediaItem({required this.artifactId, required this.owner, this.reason = ''});

  final String artifactId;

  /// `shot <scene>/<shot>`, `character <id>` or `style`.
  final String owner;
  final String reason;
}

/// What collecting would let go, as a dry run worked it out.
final class KinetoCollection {
  const KinetoCollection({required this.items, required this.bytes});

  final List<KinetoMediaItem> items;

  /// The bytes freed: a file shared with one that is kept frees nothing.
  final int bytes;
}

/// The project's media as last checked.
final class KinetoStorage {
  const KinetoStorage({
    required this.usageBytes,
    required this.recentTakes,
    this.owners = const <(String, int)>[],
    this.plan,
    this.missing = const <KinetoMediaItem>[],
    this.mismatched = const <KinetoMediaItem>[],
    this.problem,
  });

  /// The disk the media takes, each file counted once.
  final int usageBytes;

  /// How many of each owner's latest takes are kept.
  final int recentTakes;

  /// Each owner and the disk its media takes.
  final List<(String, int)> owners;
  final KinetoCollection? plan;

  /// Files a record keeps that are gone.
  final List<KinetoMediaItem> missing;

  /// Files whose bytes are not what their record says, as the last check
  /// found them.
  final List<KinetoMediaItem> mismatched;
  final String? problem;
}

/// A shot in play that is not in the package, and why.
final class KinetoExportMissing {
  const KinetoExportMissing({required this.sceneId, required this.shotId, required this.reason});

  final String sceneId;
  final String shotId;
  final String reason;
}

/// The package this session last wrote.
final class KinetoExported {
  const KinetoExported({required this.path, required this.shots, required this.complete, required this.bytes});

  final String path;
  final int shots;
  final bool complete;
  final int bytes;
}

/// Which shots can be exported, and the last package.
final class KinetoExportState {
  const KinetoExportState({
    this.ready = 0,
    this.missing = const <KinetoExportMissing>[],
    this.last,
  });

  final int ready;
  final List<KinetoExportMissing> missing;
  final KinetoExported? last;

  /// Every shot in play can be exported.
  bool get complete => missing.isEmpty && ready > 0;
}

extension KinetoProjectHandoff on KinetoProjectSession {
  /// Generate shot [shotIndex]'s video: [count] candidates, with the
  /// director's [note], branching from the take [branchFrom]. Completes
  /// once each candidate's task is accepted; videos arrive on
  /// [videoEvents].
  Future<void> generateShotVideos(
    int shotIndex, {
    int count = 1,
    String? note,
    String? branchFrom,
    bool resubmit = false,
  }) =>
      _submit(shotIndex, (token) {
        final arena = Arena();
        try {
          final noteBytes = _encodeUtf8(arena, note ?? '');
          final branchBytes = _encodeUtf8(arena, branchFrom ?? '');
          return _kinetoProjectShotGenerateVideos(
            _handle,
            shotIndex,
            count,
            noteBytes.pointer,
            noteBytes.length,
            branchBytes.pointer,
            branchBytes.length,
            resubmit,
            token,
          );
        } finally {
          arena.releaseAll();
        }
      });

  /// Choose video [video] of shot [shotIndex]'s latest generation to lock.
  Future<void> selectShotVideo(int shotIndex, int video) =>
      _submit(shotIndex, (token) => _kinetoProjectShotSelectVideo(_handle, shotIndex, video, token));

  /// Every video shot [shotIndex] has had, oldest generation first.
  List<KinetoVideoTake> videoTakes(int shotIndex) {
    _checkShotIndex(shotIndex);
    final reader = _HandoffReader(_handle);
    try {
      return reader.videoTakes(shotIndex);
    } finally {
      reader.release();
    }
  }

  /// Every scene with shots in play, and where each shot stands.
  List<KinetoSceneProgress> get sceneProgress {
    _ensureOpen();
    final reader = _HandoffReader(_handle);
    try {
      return reader.scenes();
    } finally {
      reader.release();
    }
  }

  /// Work out what collecting would let go, touching nothing.
  Future<void> planCollection() => _storage('plan_collection');

  /// Let go of what the last dry run listed, and nothing else.
  Future<void> collectMedia() => _storage('collect');

  /// Read every kept file and check its bytes against its record.
  Future<void> verifyMedia() => _storage('verify');

  Future<void> _storage(String command) {
    final contract = KinetoNativeContract.instance;
    return _submitDocument(
      (token) => _kinetoProjectStorage(_handle, contract.code(contract.storageCommands, command), token),
    );
  }

  /// The project's media as last checked.
  KinetoStorage get storage {
    _ensureOpen();
    final reader = _HandoffReader(_handle);
    try {
      return reader.storage();
    } finally {
      reader.release();
    }
  }

  /// Write a production package named [name] in folder [destination], with
  /// only the ready shots when [partial], replacing one of that name when
  /// [replace].
  Future<void> exportPackage({
    required String destination,
    required String name,
    bool partial = false,
    bool replace = false,
  }) =>
      _submitDocument((token) {
        final arena = Arena();
        try {
          final folder = _encodeUtf8(arena, destination);
          final package = _encodeUtf8(arena, name);
          return _kinetoProjectExport(
            _handle,
            folder.pointer,
            folder.length,
            package.pointer,
            package.length,
            partial,
            replace,
            token,
          );
        } finally {
          arena.releaseAll();
        }
      });

  /// Which shots can be exported, and the package this session last wrote.
  KinetoExportState get exportState {
    _ensureOpen();
    final reader = _HandoffReader(_handle);
    try {
      return reader.exportState();
    } finally {
      reader.release();
    }
  }
}

/// Reads takes, progress, storage and export state field by field.
final class _HandoffReader {
  _HandoffReader(this._handle);

  final Pointer<Void> _handle;
  final Arena _arena = Arena();
  final KinetoNativeContract _contract = KinetoNativeContract.instance;
  late final Pointer<Uint32> _count = _arena<Uint32>();
  late final Pointer<Uint64> _value = _arena<Uint64>();

  void release() => _arena.releaseAll();

  int _takeCount(String what, int shot, [int take = 0]) {
    _checkProjectResult(
      _kinetoProjectVideoTakeCount(_handle, _contract.code(_contract.videoTakeCounts, what), shot, take, _count),
    );
    return _count.value;
  }

  int _takeValue(String field, int shot, int take) {
    _checkProjectResult(
      _kinetoProjectVideoTakeValue(_handle, _contract.code(_contract.videoTakeValues, field), shot, take, _value),
    );
    return _value.value;
  }

  String _takeText(String field, int shot, int take, [int item = 0]) {
    final code = _contract.code(_contract.videoTakeTexts, field);
    return _copyProjectUtf8(
      _handle,
      (session, buffer, capacity, length) =>
          _kinetoProjectVideoTakeTextCopy(session, code, shot, take, item, buffer, capacity, length),
      field,
    );
  }

  List<KinetoVideoTake> videoTakes(int shot) => <KinetoVideoTake>[
        for (var take = 0; take < _takeCount('video_history', shot); take++)
          KinetoVideoTake(
            id: _takeText('video_id', shot, take),
            generation: _takeValue('video_generation', shot, take),
            candidate: switch (_takeValue('video_candidate', shot, take)) {
              0 => null,
              final place => place - 1,
            },
            status: _byWireName(
              KinetoTakeStatus.values,
              (status) => status.wireName,
              _contract.nameOf(_contract.takeStatuses, _takeValue('video_status', shot, take)),
            ),
            mediaPath: switch (_takeText('video_media', shot, take)) {
              '' => null,
              final path => path,
            },
            provider: _takeText('video_provider', shot, take),
            model: _takeText('video_model', shot, take),
            notes: <String>[
              for (var note = 0; note < _takeCount('video_notes', shot, take); note++)
                _takeText('video_note', shot, take, note),
            ],
            branchedFrom: switch (_takeText('video_branched_from', shot, take)) {
              '' => null,
              final take => take,
            },
            stale: <String>[
              for (var reason = 0; reason < _takeCount('video_stale', shot, take); reason++)
                _takeText('video_stale_reason', shot, take, reason),
            ],
          ),
      ];

  int _progressCount(int scene) {
    _checkProjectResult(_kinetoProjectProgressCount(_handle, scene, _count));
    return _count.value;
  }

  int _progressValue(String field, int scene, [int shot = 0]) {
    _checkProjectResult(
      _kinetoProjectProgressValue(_handle, _contract.code(_contract.progressValues, field), scene, shot, _value),
    );
    return _value.value;
  }

  String _progressText(String field, int scene, [int shot = 0]) {
    final code = _contract.code(_contract.progressTexts, field);
    return _copyProjectUtf8(
      _handle,
      (session, buffer, capacity, length) =>
          _kinetoProjectProgressTextCopy(session, code, scene, shot, buffer, capacity, length),
      field,
    );
  }

  KinetoStage _stage(int code) =>
      _byWireName(KinetoStage.values, (stage) => stage.wireName, _contract.nameOf(_contract.stages, code));

  List<KinetoSceneProgress> scenes() => <KinetoSceneProgress>[
        for (var scene = 0; scene < _progressCount(0xFFFFFFFF); scene++)
          KinetoSceneProgress(
            sceneId: _progressText('scene_id', scene),
            complete: _progressValue('scene_complete', scene) != 0,
            shots: <KinetoShotProgress>[
              for (var shot = 0; shot < _progressCount(scene); shot++)
                KinetoShotProgress(
                  shotId: _progressText('shot_id', scene, shot),
                  keyframe: _stage(_progressValue('keyframe_stage', scene, shot)),
                  video: _stage(_progressValue('video_stage', scene, shot)),
                ),
            ],
          ),
      ];

  int _mediaValue(String field) {
    _checkProjectResult(_kinetoProjectMediaValue(_handle, _contract.code(_contract.mediaValues, field), _value));
    return _value.value;
  }

  int _mediaCount(String list) {
    _checkProjectResult(_kinetoProjectMediaCount(_handle, _contract.code(_contract.mediaLists, list), _count));
    return _count.value;
  }

  String _mediaText(String list, int index, String field) {
    final listCode = _contract.code(_contract.mediaLists, list);
    final fieldCode = _contract.code(_contract.mediaTexts, field);
    return _copyProjectUtf8(
      _handle,
      (session, buffer, capacity, length) =>
          _kinetoProjectMediaTextCopy(session, listCode, index, fieldCode, buffer, capacity, length),
      field,
    );
  }

  List<KinetoMediaItem> _items(String list, {bool reasons = false}) => <KinetoMediaItem>[
        for (var index = 0; index < _mediaCount(list); index++)
          KinetoMediaItem(
            artifactId: _mediaText(list, index, 'media_artifact'),
            owner: _mediaText(list, index, 'media_owner'),
            reason: reasons ? _mediaText(list, index, 'media_reason') : '',
          ),
      ];

  KinetoStorage storage() => KinetoStorage(
        usageBytes: _mediaValue('usage_bytes'),
        recentTakes: _mediaValue('recent_takes'),
        owners: <(String, int)>[
          for (var index = 0; index < _mediaCount('owners'); index++)
            (
              _mediaText('owners', index, 'media_owner'),
              () {
                _checkProjectResult(_kinetoProjectMediaOwnerBytes(_handle, index, _value));
                return _value.value;
              }(),
            ),
        ],
        plan: _mediaValue('planned') == 0
            ? null
            : KinetoCollection(items: _items('plan', reasons: true), bytes: _mediaValue('plan_bytes')),
        missing: _items('missing'),
        mismatched: _items('mismatched'),
        problem: _mediaValue('media_damaged') == 0 ? null : _mediaText('plan', 0, 'media_problem'),
      );

  int _exportValue(String field) {
    _checkProjectResult(_kinetoProjectExportValue(_handle, _contract.code(_contract.exportValues, field), _value));
    return _value.value;
  }

  String _exportText(String field, [int index = 0]) {
    final code = _contract.code(_contract.exportTexts, field);
    return _copyProjectUtf8(
      _handle,
      (session, buffer, capacity, length) =>
          _kinetoProjectExportTextCopy(session, code, index, buffer, capacity, length),
      field,
    );
  }

  KinetoExportState exportState() => KinetoExportState(
        ready: _exportValue('export_ready'),
        missing: <KinetoExportMissing>[
          for (var index = 0; index < _exportValue('export_missing'); index++)
            KinetoExportMissing(
              sceneId: _exportText('missing_scene', index),
              shotId: _exportText('missing_shot', index),
              reason: _exportText('missing_reason', index),
            ),
        ],
        last: _exportValue('last_export') == 0
            ? null
            : KinetoExported(
                path: _exportText('last_path'),
                shots: _exportValue('last_shots'),
                complete: _exportValue('last_complete') != 0,
                bytes: _exportValue('last_bytes'),
              ),
      );
}
