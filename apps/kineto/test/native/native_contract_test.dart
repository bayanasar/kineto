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

  test('handoff codes match the native tables name for name', () {
    expect(<String>{for (final stage in KinetoStage.values) stage.wireName}, contract.stages.keys.toSet());
    expect(
      contract.storageCommands.keys.toSet(),
      <String>{'plan_collection', 'collect', 'verify'},
    );
    expect(contract.mediaLists.keys.toSet(), <String>{'plan', 'missing', 'mismatched', 'owners'});
    expect(contract.videoTakeCounts.keys.toSet(), <String>{'video_history', 'video_notes', 'video_stale'});
    expect(contract.exportTexts.keys, containsAll(<String>['missing_reason', 'last_path']));
  });

  test('video provider codes match the native table', () {
    expect(
      <String, int>{
        for (final kind in KinetoVideoProviderKind.values) kind.wireName: kind.wireValue,
      },
      contract.videoProviders,
    );
  });

  test('built-in keys, key states, settings and agents match the native tables', () {
    expect(
      KinetoKeySlot.builtIn.map((slot) => slot.name).toSet(),
      contract.keyProviders.keys.toSet(),
    );
    expect(contract.keyStates.keys.toSet(), <String>{'stored', 'environment'});
    expect(
      KinetoProviderSetting.values.map((setting) => setting.wireName).toSet(),
      contract.providerSettings.keys.toSet(),
    );
    expect(KinetoAgent.values.map((agent) => agent.wireName).toSet(), contract.agents.keys.toSet());
    expect(
      KinetoProtocol.values.map((protocol) => protocol.contractName).toSet(),
      contract.connectionProtocols.keys.toSet(),
    );
    expect(contract.connectionFields.keys.toSet(), <String>{
      'name',
      'protocol',
      'url',
      'model',
      'key',
      'price_per_million_input',
      'price_per_million_output',
      'price_per_image',
      'references',
    });
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
      KinetoProjectException.needsLockedBible: 'needs_locked_bible',
      KinetoProjectException.needsShotPlan: 'needs_shot_plan',
      KinetoProjectException.providerIncapable: 'provider_incapable',
      KinetoProjectException.mediaUnreadable: 'media_unreadable',
      KinetoProjectException.noCollectionPlan: 'no_collection_plan',
      KinetoProjectException.exportIncomplete: 'export_incomplete',
      KinetoProjectException.exportExists: 'export_exists',
      KinetoProjectException.nothingToExport: 'nothing_to_export',
      KinetoProjectException.mediaChanged: 'media_changed',
      KinetoProjectException.needsLockedCast: 'needs_locked_cast',
      KinetoProjectException.needsLockedVoice: 'needs_locked_voice',
      KinetoProjectException.pluginConfigInvalid: 'plugin_config_invalid',
      KinetoProjectException.pluginUnavailable: 'plugin_unavailable',
      KinetoProjectException.mcpNotAllowed: 'mcp_not_allowed',
      KinetoProjectException.invalidReference: 'invalid_reference',
      KinetoProjectException.plannedDirection: 'planned_direction',
      KinetoProjectException.keychainUnavailable: 'keychain_unavailable',
      KinetoProjectException.keychainFailed: 'keychain_failed',
      KinetoProjectException.styleReferenceUnusable: 'style_reference_unusable',
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

  test('voice and preference codes match the native tables name for name', () {
    Set<String> names<T>(Iterable<T> values, String Function(T) wireName) =>
        <String>{for (final value in values) wireName(value)};
    expect(names(KinetoEmotion.values, (emotion) => emotion.wireName), contract.emotions.keys.toSet());
    expect(names(KinetoDimension.values, (dimension) => dimension.wireName), contract.dimensions.keys.toSet());
    expect(names(KinetoStance.values, (stance) => stance.wireName), contract.stances.keys.toSet());
    expect(
      names(KinetoDurationSource.values, (source) => source.wireName),
      contract.durationSources.keys.toSet(),
    );
    expect(
      names(KinetoSpeechProviderKind.values, (kind) => kind.wireName),
      contract.speechProviders.keys.toSet(),
    );
    expect(contract.suggestionDecisions.keys.toSet(), <String>{'accept', 'reject'});
    expect(contract.planValues.keys, containsAll(<String>['shot_dialogue_millis', 'shot_too_short']));
    expect(contract.voiceCounts, isNotEmpty);
    expect(contract.voiceValues, isNotEmpty);
    expect(contract.voiceTexts, isNotEmpty);
  });

  test('every plugin field the bindings read is in the native tables', () {
    expect(contract.pluginValues.keys.toSet(), kinetoPluginValueFields);
    expect(contract.pluginTexts.keys.toSet(), kinetoPluginTextFields);
  });

  test('reference and MCP codes match the native tables name for name', () {
    expect(
      <String>{for (final kind in KinetoToolArgumentKind.values) kind.wireName},
      contract.toolArgumentKinds.keys.toSet(),
    );
    expect(
      contract.referenceTexts.keys.toSet(),
      <String>{'id', 'server', 'tool', 'arguments', 'text', 'attached_to', 'content_hash'},
    );
    expect(contract.mcpValues.keys.toSet(), <String>{'enabled', 'listed', 'required', 'kind'});
    expect(contract.mcpTexts.keys, containsAll(<String>['tool_name', 'parameter_name', 'problem']));
  });
}
