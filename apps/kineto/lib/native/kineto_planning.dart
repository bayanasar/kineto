part of 'kineto_engine.dart';

@Native<Int32 Function(Pointer<Void>, Bool, Uint64)>(symbol: 'kineto_project_plan_shots')
external int _kinetoProjectPlanShots(Pointer<Void> session, bool resubmit, int token);

@Native<Int32 Function(Pointer<Void>, Pointer<Uint8>, Uint64, Pointer<Uint8>, Uint64, Bool, Uint64)>(
  symbol: 'kineto_project_replan_scene',
)
external int _kinetoProjectReplanScene(
  Pointer<Void> session,
  Pointer<Uint8> sceneId,
  int sceneIdLength,
  Pointer<Uint8> direction,
  int directionLength,
  bool resubmit,
  int token,
);

/// One shot of an edited scene, laid out as `KinetoPlannedShot`.
final class _NativePlannedShot extends Struct {
  external Pointer<Uint8> id;

  @Uint64()
  external int idLength;

  external Pointer<Uint8> purpose;

  @Uint64()
  external int purposeLength;

  @Uint32()
  external int direction;

  @Uint32()
  external int framing;

  external Pointer<Uint8> lens;

  @Uint64()
  external int lensLength;

  @Uint32()
  external int cameraMotion;

  external Pointer<Uint8> action;

  @Uint64()
  external int actionLength;

  external Pointer<Uint8> dialogue;

  @Uint64()
  external int dialogueLength;

  external Pointer<Uint8> emotion;

  @Uint64()
  external int emotionLength;

  @Double()
  external double durationTarget;

  external Pointer<_NativeText> characters;

  @Uint64()
  external int characterCount;

  external Pointer<_NativeText> continuity;

  @Uint64()
  external int continuityCount;

  external Pointer<Uint8> continuityFrom;

  @Uint64()
  external int continuityFromLength;
}

@Native<Int32 Function(Pointer<Void>, Uint32, Pointer<Uint8>, Uint64, Pointer<_NativePlannedShot>, Uint64, Uint64)>(
  symbol: 'kineto_project_save_scene_plan',
)
external int _kinetoProjectSaveScenePlan(
  Pointer<Void> session,
  int basedOn,
  Pointer<Uint8> sceneId,
  int sceneIdLength,
  Pointer<_NativePlannedShot> shots,
  int shotCount,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint64)>(symbol: 'kineto_project_lock_shot_plan')
external int _kinetoProjectLockShotPlan(Pointer<Void> session, int revision, int token);

@Native<Int32 Function(Pointer<Void>, Uint32, Pointer<Uint8>, Uint64, Bool, Bool, Uint64)>(
  symbol: 'kineto_project_shot_refine',
)
external int _kinetoProjectShotRefine(
  Pointer<Void> session,
  int shotIndex,
  Pointer<Uint8> note,
  int noteLength,
  bool riskPayingTwice,
  bool identityDrift,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Pointer<Uint64>)>(symbol: 'kineto_project_plan_state')
external int _kinetoProjectPlanState(Pointer<Void> session, Pointer<Uint64> out);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Uint32, Pointer<Uint32>)>(
  symbol: 'kineto_project_plan_count',
)
external int _kinetoProjectPlanCount(
  Pointer<Void> session,
  int what,
  int group,
  int item,
  Pointer<Uint32> out,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Uint32, Pointer<Uint64>)>(
  symbol: 'kineto_project_plan_value',
)
external int _kinetoProjectPlanValue(
  Pointer<Void> session,
  int field,
  int group,
  int item,
  Pointer<Uint64> out,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Uint32, Uint32, Pointer<Uint8>, Uint64, Pointer<Uint64>)>(
  symbol: 'kineto_project_plan_text_copy',
)
external int _kinetoProjectPlanTextCopy(
  Pointer<Void> session,
  int field,
  int group,
  int item,
  int sub,
  Pointer<Uint8> outBuffer,
  int outCapacity,
  Pointer<Uint64> outLength,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Uint32, Pointer<Uint32>)>(
  symbol: 'kineto_project_take_count',
)
external int _kinetoProjectTakeCount(
  Pointer<Void> session,
  int what,
  int shotIndex,
  int take,
  Pointer<Uint32> out,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Uint32, Pointer<Uint64>)>(
  symbol: 'kineto_project_take_value',
)
external int _kinetoProjectTakeValue(
  Pointer<Void> session,
  int field,
  int shotIndex,
  int take,
  Pointer<Uint64> out,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Uint32, Uint32, Pointer<Uint8>, Uint64, Pointer<Uint64>)>(
  symbol: 'kineto_project_take_text_copy',
)
external int _kinetoProjectTakeTextCopy(
  Pointer<Void> session,
  int field,
  int shotIndex,
  int take,
  int reason,
  Pointer<Uint8> outBuffer,
  int outCapacity,
  Pointer<Uint64> outLength,
);

enum KinetoFraming {
  extremeWide('extreme_wide', 'Extreme wide'),
  wide('wide', 'Wide'),
  full('full', 'Full'),
  mediumWide('medium_wide', 'Medium wide'),
  medium('medium', 'Medium'),
  mediumCloseUp('medium_close_up', 'Medium close-up'),
  closeUp('close_up', 'Close-up'),
  extremeCloseUp('extreme_close_up', 'Extreme close-up'),
  overTheShoulder('over_the_shoulder', 'Over the shoulder'),
  insert('insert', 'Insert');

  const KinetoFraming(this.wireName, this.label);

  final String wireName;
  final String label;
}

enum KinetoCameraMotion {
  still('static', 'Static'),
  slowDollyIn('slow_dolly_in', 'Slow dolly in'),
  dollyIn('dolly_in', 'Dolly in'),
  slowDollyOut('slow_dolly_out', 'Slow dolly out'),
  dollyOut('dolly_out', 'Dolly out'),
  panLeft('pan_left', 'Pan left'),
  panRight('pan_right', 'Pan right'),
  tiltUp('tilt_up', 'Tilt up'),
  tiltDown('tilt_down', 'Tilt down'),
  tracking('tracking', 'Tracking'),
  handheld('handheld', 'Handheld');

  const KinetoCameraMotion(this.wireName, this.label);

  final String wireName;
  final String label;
}

/// One planned shot.
final class KinetoPlannedShot {
  const KinetoPlannedShot({
    required this.id,
    required this.purpose,
    required this.direction,
    required this.framing,
    required this.lens,
    required this.cameraMotion,
    required this.action,
    required this.emotion,
    required this.durationTarget,
    this.dialogue = '',
    this.characters = const <String>[],
    this.continuity = const <String>[],
    this.continuityFrom,
  });

  final String id;

  /// What the shot is for in the scene.
  final String purpose;
  final KinetoShotDirection direction;
  final KinetoFraming framing;

  /// Focal length, such as `50mm`.
  final String lens;
  final KinetoCameraMotion cameraMotion;
  final String action;

  /// What is said in the shot; empty when nothing is.
  final String dialogue;
  final String emotion;

  /// Seconds.
  final double durationTarget;

  /// Cast IDs of those seen.
  final List<String> characters;

  /// What must stay as it was.
  final List<String> continuity;

  /// The earlier shot this one continues from.
  final String? continuityFrom;

  KinetoPlannedShot copyWith({
    String? purpose,
    KinetoShotDirection? direction,
    KinetoFraming? framing,
    String? lens,
    KinetoCameraMotion? cameraMotion,
    String? action,
    String? dialogue,
    String? emotion,
    double? durationTarget,
  }) =>
      KinetoPlannedShot(
        id: id,
        purpose: purpose ?? this.purpose,
        direction: direction ?? this.direction,
        framing: framing ?? this.framing,
        lens: lens ?? this.lens,
        cameraMotion: cameraMotion ?? this.cameraMotion,
        action: action ?? this.action,
        dialogue: dialogue ?? this.dialogue,
        emotion: emotion ?? this.emotion,
        durationTarget: durationTarget ?? this.durationTarget,
        characters: characters,
        continuity: continuity,
        continuityFrom: continuityFrom,
      );
}

/// The shots of one scene.
final class KinetoScenePlan {
  const KinetoScenePlan({required this.sceneId, required this.shots});

  final String sceneId;
  final List<KinetoPlannedShot> shots;
}

/// Why a shot plan may no longer follow the locked screenplay and bible.
enum KinetoPlanStaleCause {
  notLocked('not_locked'),
  sceneChanged('scene_changed'),
  sceneRemoved('scene_removed'),
  sceneAdded('scene_added'),
  entry('entry');

  const KinetoPlanStaleCause(this.wireName);

  final String wireName;
}

final class KinetoPlanStaleness {
  const KinetoPlanStaleness(this.cause, [this.subject = '']);

  final KinetoPlanStaleCause cause;
  final String subject;

  String get description => switch (cause) {
        KinetoPlanStaleCause.notLocked => 'the screenplay or the bible is not locked',
        KinetoPlanStaleCause.sceneChanged => 'scene $subject changed',
        KinetoPlanStaleCause.sceneRemoved => 'scene $subject was removed',
        KinetoPlanStaleCause.sceneAdded => 'scene $subject is new',
        KinetoPlanStaleCause.entry => '$subject changed',
      };
}

/// Where a recipe step stands.
enum KinetoStepState {
  waiting('waiting', 'Waiting'),
  ready('ready', 'Ready'),
  awaitingApproval('awaiting_approval', 'Awaiting approval'),
  done('done', 'Done');

  const KinetoStepState(this.wireName, this.label);

  final String wireName;
  final String label;
}

/// One step of the project's recipe.
final class KinetoWorkflowStep {
  const KinetoWorkflowStep({
    required this.id,
    required this.description,
    required this.state,
    this.approval = false,
    this.waitingOn = const <String>[],
    this.stale = const <String>[],
  });

  final String id;
  final String description;
  final KinetoStepState state;

  /// Whether the director approves what it made.
  final bool approval;

  /// The steps it follows that are not done.
  final List<String> waitingOn;

  /// What changed since its work was made.
  final List<String> stale;
}

/// The shot plan and where the project stands in its recipe, as the engine
/// last published them.
final class KinetoPlanning {
  const KinetoPlanning({
    this.revision,
    this.scenes = const <KinetoScenePlan>[],
    this.revisions = const <KinetoRevision>[],
    this.lockedRevision,
    this.stale = const <KinetoPlanStaleness>[],
    this.steps = const <KinetoWorkflowStep>[],
    this.workflowProblem,
  });

  /// The latest plan revision still in play.
  final int? revision;
  final List<KinetoScenePlan> scenes;

  /// Every plan revision, oldest first.
  final List<KinetoRevision> revisions;

  /// The revision whose shots are in play.
  final int? lockedRevision;
  final List<KinetoPlanStaleness> stale;
  final List<KinetoWorkflowStep> steps;

  /// Why the steps could not be worked out, when they could not.
  final String? workflowProblem;

  bool get planLocked => revision != null && lockedRevision == revision;

  /// The planned shot named [shotId], if the plan in play has it.
  KinetoPlannedShot? shot(String shotId) {
    for (final scene in scenes) {
      for (final shot in scene.shots) {
        if (shot.id == shotId) return shot;
      }
    }
    return null;
  }
}

/// A take's standing in its shot.
enum KinetoTakeStatus {
  draft('draft'),
  candidate('candidate'),
  selected('selected'),
  locked('locked'),
  superseded('superseded');

  const KinetoTakeStatus(this.wireName);

  final String wireName;
}

/// One keyframe candidate of a shot, current or superseded.
final class KinetoTake {
  const KinetoTake({
    required this.id,
    required this.generation,
    required this.status,
    this.mediaPath,
    this.provider = '',
    this.model = '',
    this.refined = false,
  });

  final String id;

  /// The generation it came from.
  final int generation;
  final KinetoTakeStatus status;

  /// Absolute path of its image; null for a demo take.
  final String? mediaPath;
  final String provider;
  final String model;

  /// Made as the director's note asked.
  final bool refined;
}

/// Shot plan, recipe and take commands and reads. Every command queues on
/// the project's worker and completes when it has run; reads return
/// published state and never do I/O.
extension KinetoProjectPlanning on KinetoProjectSession {
  /// Plan the shots of the locked screenplay with the session's language
  /// model, from it and the locked bible.
  Future<void> planShots({bool resubmit = false}) =>
      _submitDocument((token) => _kinetoProjectPlanShots(_handle, resubmit, token));

  /// Plan scene [sceneId] of the latest plan again, as [direction] asks.
  Future<void> replanScene(String sceneId, {String? direction, bool resubmit = false}) =>
      _submitDocument((token) {
        final arena = Arena();
        try {
          final scene = _encodeUtf8(arena, sceneId);
          final directionBytes = _encodeUtf8(arena, direction ?? '');
          return _kinetoProjectReplanScene(
            _handle,
            scene.pointer,
            scene.length,
            directionBytes.pointer,
            directionBytes.length,
            resubmit,
            token,
          );
        } finally {
          arena.releaseAll();
        }
      });

  /// Save the director's edit of [scene]'s shots of plan revision
  /// [basedOn], which must be the latest.
  Future<void> saveScenePlan(int basedOn, KinetoScenePlan scene) {
    final contract = KinetoNativeContract.instance;
    return _submitDocument((token) {
      final arena = Arena();
      try {
        Pointer<_NativeText> texts(List<String> values) {
          final native = arena<_NativeText>(values.isEmpty ? 1 : values.length);
          for (var index = 0; index < values.length; index++) {
            final text = _encodeUtf8(arena, values[index]);
            native[index]
              ..pointer = text.pointer
              ..length = text.length;
          }
          return native;
        }

        final shots = arena<_NativePlannedShot>(scene.shots.isEmpty ? 1 : scene.shots.length);
        for (var index = 0; index < scene.shots.length; index++) {
          final shot = scene.shots[index];
          final id = _encodeUtf8(arena, shot.id);
          final purpose = _encodeUtf8(arena, shot.purpose);
          final lens = _encodeUtf8(arena, shot.lens);
          final action = _encodeUtf8(arena, shot.action);
          final dialogue = _encodeUtf8(arena, shot.dialogue);
          final emotion = _encodeUtf8(arena, shot.emotion);
          final from = _encodeUtf8(arena, shot.continuityFrom ?? '');
          shots[index]
            ..id = id.pointer
            ..idLength = id.length
            ..purpose = purpose.pointer
            ..purposeLength = purpose.length
            ..direction = contract.code(contract.directions, shot.direction.wireName)
            ..framing = contract.code(contract.framings, shot.framing.wireName)
            ..lens = lens.pointer
            ..lensLength = lens.length
            ..cameraMotion = contract.code(contract.cameraMotions, shot.cameraMotion.wireName)
            ..action = action.pointer
            ..actionLength = action.length
            ..dialogue = dialogue.pointer
            ..dialogueLength = dialogue.length
            ..emotion = emotion.pointer
            ..emotionLength = emotion.length
            ..durationTarget = shot.durationTarget
            ..characters = texts(shot.characters)
            ..characterCount = shot.characters.length
            ..continuity = texts(shot.continuity)
            ..continuityCount = shot.continuity.length
            ..continuityFrom = from.pointer
            ..continuityFromLength = from.length;
        }
        final sceneId = _encodeUtf8(arena, scene.sceneId);
        return _kinetoProjectSaveScenePlan(
          _handle,
          basedOn,
          sceneId.pointer,
          sceneId.length,
          shots,
          scene.shots.length,
          token,
        );
      } finally {
        arena.releaseAll();
      }
    });
  }

  /// Lock plan [revision]: its shots become the ones in play.
  Future<void> lockShotPlan(int revision) =>
      _submitDocument((token) => _kinetoProjectLockShotPlan(_handle, revision, token));

  /// Generate shot [shotIndex]'s keyframes again as [note] asks.
  Future<void> refineShot(
    int shotIndex,
    String note, {
    bool riskPayingTwice = false,
    bool identityDrift = false,
  }) =>
      _submit(shotIndex, (token) {
        final arena = Arena();
        try {
          final bytes = _encodeUtf8(arena, note);
          return _kinetoProjectShotRefine(
            _handle,
            shotIndex,
            bytes.pointer,
            bytes.length,
            riskPayingTwice,
            identityDrift,
            token,
          );
        } finally {
          arena.releaseAll();
        }
      });

  /// The shot plan and the recipe's steps as last published.
  KinetoPlanning get planning {
    _ensureOpen();
    final reader = _PlanningReader(_handle);
    try {
      return reader.read();
    } finally {
      reader.release();
    }
  }

  /// Every take shot [shotIndex] has had, oldest generation first.
  List<KinetoTake> takes(int shotIndex) {
    _checkShotIndex(shotIndex);
    final reader = _PlanningReader(_handle);
    try {
      return reader.takes(shotIndex);
    } finally {
      reader.release();
    }
  }

  /// Why each current take of shot [shotIndex] is stale, in take order.
  List<List<String>> staleReasons(int shotIndex) {
    _checkShotIndex(shotIndex);
    final reader = _PlanningReader(_handle);
    try {
      return reader.staleReasons(shotIndex, shotSnapshot(shotIndex).candidateCount);
    } finally {
      reader.release();
    }
  }
}

/// Reads the published plan, recipe and takes field by field.
final class _PlanningReader {
  _PlanningReader(this._handle);

  final Pointer<Void> _handle;
  final Arena _arena = Arena();
  final KinetoNativeContract _contract = KinetoNativeContract.instance;
  late final Pointer<Uint32> _count = _arena<Uint32>();
  late final Pointer<Uint64> _value = _arena<Uint64>();

  void release() => _arena.releaseAll();

  int count(String what, {int group = 0, int item = 0}) {
    _checkProjectResult(
      _kinetoProjectPlanCount(_handle, _contract.code(_contract.planCounts, what), group, item, _count),
    );
    return _count.value;
  }

  int value(String field, {int group = 0, int item = 0}) {
    _checkProjectResult(
      _kinetoProjectPlanValue(_handle, _contract.code(_contract.planValues, field), group, item, _value),
    );
    return _value.value;
  }

  String text(String field, {int group = 0, int item = 0, int sub = 0}) {
    final code = _contract.code(_contract.planTexts, field);
    return _copyProjectUtf8(
      _handle,
      (session, buffer, capacity, length) =>
          _kinetoProjectPlanTextCopy(session, code, group, item, sub, buffer, capacity, length),
      field,
    );
  }

  T named<T extends Enum>(List<T> values, String Function(T) wireName, Map<String, int> table, int code) =>
      _byWireName(values, wireName, _contract.nameOf(table, code));

  KinetoPlanning read() {
    _checkProjectResult(_kinetoProjectPlanState(_handle, _value));
    final bits = _value.value;
    int? revision(String field) {
      final value = _contract.planField(bits, field);
      return value == 0 ? null : value;
    }

    final problem = _contract.planField(bits, 'workflow_unknown') != 0 ? text('workflow_problem') : null;
    return KinetoPlanning(
      revision: revision('plan_revision'),
      scenes: <KinetoScenePlan>[
        for (var group = 0; group < count('plan_scenes'); group++)
          KinetoScenePlan(
            sceneId: text('scene_id', group: group),
            shots: <KinetoPlannedShot>[
              for (var item = 0; item < count('plan_shots', group: group); item++) _shot(group, item),
            ],
          ),
      ],
      revisions: <KinetoRevision>[
        for (var item = 0; item < count('plan_revisions'); item++) _revision(item),
      ],
      lockedRevision: revision('plan_locked_revision'),
      stale: <KinetoPlanStaleness>[
        for (var item = 0; item < count('plan_stale_causes'); item++)
          KinetoPlanStaleness(
            named(
              KinetoPlanStaleCause.values,
              (cause) => cause.wireName,
              _contract.planStaleCauses,
              value('plan_stale_cause', item: item),
            ),
            text('plan_stale_subject', item: item),
          ),
      ],
      steps: <KinetoWorkflowStep>[
        for (var group = 0; group < count('workflow_steps'); group++) _step(group),
      ],
      workflowProblem: problem,
    );
  }

  KinetoPlannedShot _shot(int group, int item) {
    final from = text('shot_continuity_from', group: group, item: item);
    return KinetoPlannedShot(
      id: text('shot_id', group: group, item: item),
      purpose: text('shot_purpose', group: group, item: item),
      direction: KinetoShotDirection.fromWire(value('shot_direction', group: group, item: item)),
      framing: named(
        KinetoFraming.values,
        (framing) => framing.wireName,
        _contract.framings,
        value('shot_framing', group: group, item: item),
      ),
      lens: text('shot_lens', group: group, item: item),
      cameraMotion: named(
        KinetoCameraMotion.values,
        (motion) => motion.wireName,
        _contract.cameraMotions,
        value('shot_camera_motion', group: group, item: item),
      ),
      action: text('shot_action', group: group, item: item),
      dialogue: text('shot_dialogue', group: group, item: item),
      emotion: text('shot_emotion', group: group, item: item),
      durationTarget: value('shot_duration_millis', group: group, item: item) / 1000,
      characters: <String>[
        for (var sub = 0; sub < count('shot_characters', group: group, item: item); sub++)
          text('shot_character', group: group, item: item, sub: sub),
      ],
      continuity: <String>[
        for (var sub = 0; sub < count('shot_continuity', group: group, item: item); sub++)
          text('shot_continuity', group: group, item: item, sub: sub),
      ],
      continuityFrom: from.isEmpty ? null : from,
    );
  }

  KinetoRevision _revision(int item) {
    final basedOn = value('plan_revision_based_on', item: item);
    return KinetoRevision(
      number: value('plan_revision_number', item: item),
      status: named(
        KinetoRevisionStatus.values,
        (status) => status.wireName,
        _contract.revisionStatuses,
        value('plan_revision_status', item: item),
      ),
      origin: named(
        KinetoRevisionOrigin.values,
        (origin) => origin.wireName,
        _contract.origins,
        value('plan_revision_origin', item: item),
      ),
      basedOn: basedOn == 0 ? null : basedOn,
      provider: text('plan_revision_provider', item: item),
      model: text('plan_revision_model', item: item),
    );
  }

  KinetoWorkflowStep _step(int group) => KinetoWorkflowStep(
        id: text('step_id', group: group),
        description: text('step_description', group: group),
        state: named(
          KinetoStepState.values,
          (state) => state.wireName,
          _contract.stepStates,
          value('step_state', group: group),
        ),
        approval: value('step_approval', group: group) != 0,
        waitingOn: <String>[
          for (var item = 0; item < count('step_waiting', group: group); item++)
            text('step_waiting', group: group, item: item),
        ],
        stale: <String>[
          for (var item = 0; item < count('step_stale', group: group); item++)
            text('step_stale', group: group, item: item),
        ],
      );

  int _takeCount(String what, int shotIndex, [int take = 0]) {
    _checkProjectResult(
      _kinetoProjectTakeCount(_handle, _contract.code(_contract.takeCounts, what), shotIndex, take, _count),
    );
    return _count.value;
  }

  int _takeValue(String field, int shotIndex, int take) {
    _checkProjectResult(
      _kinetoProjectTakeValue(_handle, _contract.code(_contract.takeValues, field), shotIndex, take, _value),
    );
    return _value.value;
  }

  String _takeText(String field, int shotIndex, int take, [int reason = 0]) {
    final code = _contract.code(_contract.takeTexts, field);
    return _copyProjectUtf8(
      _handle,
      (session, buffer, capacity, length) =>
          _kinetoProjectTakeTextCopy(session, code, shotIndex, take, reason, buffer, capacity, length),
      field,
    );
  }

  List<KinetoTake> takes(int shotIndex) => <KinetoTake>[
        for (var take = 0; take < _takeCount('history', shotIndex); take++)
          KinetoTake(
            id: _takeText('take_id', shotIndex, take),
            generation: _takeValue('take_generation', shotIndex, take),
            status: named(
              KinetoTakeStatus.values,
              (status) => status.wireName,
              _contract.takeStatuses,
              _takeValue('take_status', shotIndex, take),
            ),
            mediaPath: switch (_takeText('take_media', shotIndex, take)) {
              '' => null,
              final path => path,
            },
            provider: _takeText('take_provider', shotIndex, take),
            model: _takeText('take_model', shotIndex, take),
            refined: _takeValue('take_refined', shotIndex, take) != 0,
          ),
      ];

  List<List<String>> staleReasons(int shotIndex, int candidates) => <List<String>>[
        for (var take = 0; take < candidates; take++)
          <String>[
            for (var reason = 0; reason < _takeCount('staleness', shotIndex, take); reason++)
              _takeText('stale_reason', shotIndex, take, reason),
          ],
      ];
}
