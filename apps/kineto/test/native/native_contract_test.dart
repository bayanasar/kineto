import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/native/kineto_engine.dart';

/// These tests fail when the Rust and Dart sides of the native contract are
/// changed apart: the ABI revision cannot catch a one-sided edit, because both
/// revisions are bumped by hand in the same change.
void main() {
  final contract = KinetoNativeContract.instance;

  test('project status codes match the native table name for name', () {
    expect(kinetoProjectStatusCodes, contract.statuses);
    for (final code in contract.statuses.values.where((code) => code != 0)) {
      expect(
        kinetoProjectStatusMessages,
        contains(code),
        reason: 'status $code has no message for the director',
      );
    }
  });

  test('status constants used for control flow match the native table', () {
    expect(KinetoProjectException.alreadyExists, contract.statuses['already_exists']);
    expect(KinetoProjectException.busy, contract.statuses['busy']);
    expect(KinetoProjectException.migrationRequired, contract.statuses['migration_required']);
    expect(KinetoProjectException.lockUnavailable, contract.statuses['lock_unavailable']);
    expect(KinetoProjectException.generationJob, contract.statuses['generation_job']);
    expect(
      KinetoProjectException.providerNotConfigured,
      contract.statuses['provider_not_configured'],
    );
    expect(KinetoProjectException.shotIntentMissing, contract.statuses['shot_intent_missing']);
    expect(KinetoProjectException.providerRefused, contract.statuses['provider_refused']);
  });

  test('video state codes match the native table', () {
    expect(
      <String>{for (final state in KinetoVideoState.values) state.wireName},
      contract.videoStates.keys.toSet(),
    );
    expect(KinetoProjectException.videoInFlight, contract.statuses['video_in_flight']);
  });

  test('image provider codes match the native table', () {
    expect(
      <String, int>{
        for (final kind in KinetoImageProviderKind.values) kind.wireName: kind.wireValue,
      },
      contract.imageProviders,
    );
  });

  test('document codes match the native tables name for name', () {
    Set<String> names<T>(Iterable<T> values, String Function(T) wireName) =>
        <String>{for (final value in values) wireName(value)};
    expect(names(KinetoLanguageModelKind.values, (kind) => kind.wireName), contract.llmProviders.keys.toSet());
    expect(names(KinetoBlockKind.values, (kind) => kind.wireName), contract.blockKinds.keys.toSet());
    expect(names(KinetoSceneSetting.values, (setting) => setting.wireName), contract.settings.keys.toSet());
    expect(names(KinetoRevisionStatus.values, (status) => status.wireName), contract.revisionStatuses.keys.toSet());
    expect(names(KinetoRevisionOrigin.values, (origin) => origin.wireName), contract.origins.keys.toSet());
    expect(names(KinetoSourceFormat.values, (format) => format.wireName), contract.sourceFormats.keys.toSet());
    expect(names(KinetoDocumentKind.values, (kind) => kind.wireName), contract.documentCosts.keys.toSet());
    expect(
      contract.documentFields,
      containsAll(<String>[
        'source_revision',
        'analysis_revision',
        'screenplay_revision',
        'locked_revision',
        'analysis_stale',
        'screenplay_stale',
        'documents_damaged',
      ]),
    );
    expect(KinetoProjectException.needsAnalysis, contract.statuses['needs_analysis']);
    expect(KinetoProjectException.notLatest, contract.statuses['not_latest']);
    expect(KinetoProjectException.modelNotConfigured, contract.statuses['model_not_configured']);
    expect(KinetoProjectException.costUnknown, contract.statuses['cost_unknown']);
  });

  test('production codes match the native tables name for name', () {
    Set<String> names<T>(Iterable<T> values, String Function(T) wireName) =>
        <String>{for (final value in values) wireName(value)};
    expect(names(KinetoBibleEntryKind.values, (kind) => kind.wireName), contract.entryKinds.keys.toSet());
    expect(names(KinetoBibleStaleCause.values, (cause) => cause.wireName), contract.staleCauses.keys.toSet());
    expect(names(KinetoChoiceStatus.values, (status) => status.wireName), contract.choiceStatuses.keys.toSet());
    expect(names(KinetoCastImageKind.values, (kind) => kind.wireName), contract.castImageKinds.keys.toSet());
    expect(names(KinetoImageCostKind.values, (kind) => kind.wireName), contract.imageCosts.keys.toSet());
    expect(contract.decisions.keys.toSet(), <String>{'select', 'reject', 'lock'});
    expect(
      contract.productionFields,
      containsAll(<String>[
        'bible_revision',
        'bible_locked_revision',
        'bible_stale',
        'style_locked',
        'style_unresolved',
        'production_damaged',
      ]),
    );
    expect(names(KinetoFraming.values, (framing) => framing.wireName), contract.framings.keys.toSet());
    expect(names(KinetoCameraMotion.values, (motion) => motion.wireName), contract.cameraMotions.keys.toSet());
    expect(names(KinetoPlanStaleCause.values, (cause) => cause.wireName), contract.planStaleCauses.keys.toSet());
    expect(names(KinetoStepState.values, (state) => state.wireName), contract.stepStates.keys.toSet());
    expect(names(KinetoTakeStatus.values, (status) => status.wireName), contract.takeStatuses.keys.toSet());
    expect(
      contract.planFields,
      containsAll(<String>['plan_revision', 'plan_locked_revision', 'plan_stale', 'workflow_unknown']),
    );
    final statuses = <int, String>{
      KinetoProjectException.invalidArgument: 'invalid_argument',
      KinetoProjectException.locked: 'locked',
      KinetoProjectException.invalidDocument: 'invalid_document',
      KinetoProjectException.identityDrift: 'identity_drift',
      KinetoProjectException.needsLockedScreenplay: 'needs_locked_screenplay',
      KinetoProjectException.imageProviderUnsuitable: 'image_provider_unsuitable',
      KinetoProjectException.noSuchArtifact: 'no_such_artifact',
      KinetoProjectException.needsBible: 'needs_bible',
      KinetoProjectException.needsLockedStyle: 'needs_locked_style',
      KinetoProjectException.needsSelectedPortrait: 'needs_selected_portrait',
      KinetoProjectException.noIdentityReference: 'no_identity_reference',
      KinetoProjectException.rejected: 'rejected',
      KinetoProjectException.packIncomplete: 'pack_incomplete',
      KinetoProjectException.needsLockedBible: 'needs_locked_bible',
      KinetoProjectException.needsShotPlan: 'needs_shot_plan',
    };
    for (final MapEntry(key: code, value: name) in statuses.entries) {
      expect(code, contract.statuses[name], reason: name);
      expect(kinetoProjectStatusMessages[code], isNotNull, reason: name);
    }
  });

  test('shot direction wire codes match the native table', () {
    expect(
      <String, int>{
        for (final direction in KinetoShotDirection.values)
          direction.wireName: direction.wireValue,
      },
      contract.directions,
    );
  });

  test('a snapshot packed natively decodes to the values it was packed from', () {
    final sample = KinetoShotSnapshot.fromBits(contract.snapshotSample);

    expect(sample.generated, isTrue);
    expect(sample.locked, isFalse);
    expect(sample.stale, isTrue);
    expect(sample.migrationPending, isTrue);
    expect(sample.hasInvalidHashes, isTrue);
    expect(sample.candidateCount, 3);
    expect(sample.selectedIndex, 2);
    expect(sample.direction, KinetoShotDirection.tension);
    expect(sample.generation, 7);
    expect(sample.supersededCount, 18);
    expect(sample.videoState, KinetoVideoState.stale);
    expect(
      contract.snapshotFields,
      containsAll(<String>[
        'generated',
        'locked',
        'stale',
        'migration_pending',
        'invalid_hashes',
        'candidate_count',
        'selected_plus_one',
        'direction',
        'generation_revision',
        'superseded_count',
        'video_state',
      ]),
    );
  });
}
