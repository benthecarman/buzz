import * as React from "react";

import type { RuntimeFileConfigSubset } from "@/shared/api/tauri";
import type { EnvVarsValue } from "./EnvVarsEditor";
import { computeLocalModeGate } from "./agentConfigOptions";

/**
 * The definition dialog's readiness gate: which credential keys the chosen
 * harness still needs, and whether Save may proceed.
 *
 * Extracted from `AgentDefinitionDialog` as a memo-only wrapper — the policy
 * itself stays in `computeLocalModeGate`, which the create, edit, and
 * onboarding surfaces share.
 */
export function useDefinitionLocalModeGate({
  bakedEnvKeys,
  envVars,
  globalEnvVars,
  globalModel,
  globalProvider,
  model,
  provider,
  runtimeId,
  runtimeFileConfig,
}: {
  bakedEnvKeys: string[] | undefined;
  envVars: EnvVarsValue;
  globalEnvVars: Record<string, string>;
  globalModel: string;
  globalProvider: string;
  model: string;
  /** Already trimmed by the caller. */
  provider: string;
  runtimeId: string;
  runtimeFileConfig: RuntimeFileConfigSubset | null | undefined;
}) {
  return React.useMemo(
    () =>
      computeLocalModeGate({
        bakedEnvKeys,
        envVars,
        globalEnvVars,
        globalProvider,
        globalModel,
        isProviderMode: false,
        model,
        provider,
        runtimeId,
        runtimeFileConfig,
      }),
    [
      bakedEnvKeys,
      envVars,
      globalEnvVars,
      globalModel,
      globalProvider,
      model,
      provider,
      runtimeId,
      runtimeFileConfig,
    ],
  );
}
