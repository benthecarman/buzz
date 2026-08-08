import { useState } from "react";
import { LoaderCircle } from "lucide-react";
import { toast } from "sonner";

import { disableWallet, enableWallet } from "@/features/wallet/api";
import { useBitcoinCompileEnabled } from "@/features/wallet/hooks";
import { walletErrorMessage } from "@/features/wallet/lib/walletError";
import { setAgentManagedProfiles } from "@/shared/api/tauri";
import { desktopFeatures, useFeatureToggle } from "@/shared/features";
import type { FeatureDefinition } from "@/shared/features";
import { Switch } from "@/shared/ui/switch";
import { SettingsOptionGroup, SettingsOptionRow } from "./SettingsOptionGroup";
import { SettingsSectionHeader } from "./SettingsSectionHeader";

function FeatureRow({ feature }: { feature: FeatureDefinition }) {
  const [enabled, toggle] = useFeatureToggle(feature.id);
  const [applying, setApplying] = useState(false);
  const switchId = `feature-toggle-${feature.id}`;

  async function handleToggle(value: boolean) {
    if (feature.id === "bitcoin") {
      setApplying(true);
      try {
        const result = value ? await enableWallet() : await disableWallet();
        toggle(value);
        if (result.publicationWarnings.length > 0) {
          toast.warning(
            `Wallet ${value ? "enabled" : "disabled"}, but some communities could not be updated. They may not support wallet profile payments yet.`,
          );
        }
      } catch (error) {
        toast.error(walletErrorMessage(error));
      } finally {
        setApplying(false);
      }
      return;
    }

    toggle(value);
    if (feature.id === "agentManagedProfiles") {
      void setAgentManagedProfiles(value).catch((error) => {
        console.error("Failed to apply agent-managed profiles setting:", error);
      });
    }
  }

  return (
    <SettingsOptionRow>
      <div className="min-w-0 flex-1">
        <p className="text-sm font-medium" id={`${switchId}-label`}>
          {feature.name}
        </p>
        <p className="text-xs text-muted-foreground/70" data-settings-subcopy>
          {feature.description}
        </p>
      </div>
      <div className="flex items-center gap-2">
        {applying ? (
          <LoaderCircle className="h-4 w-4 animate-spin text-muted-foreground" />
        ) : null}
        <Switch
          aria-labelledby={`${switchId}-label`}
          checked={enabled}
          data-testid={switchId}
          disabled={applying}
          onCheckedChange={(value) => void handleToggle(value)}
        />
      </div>
    </SettingsOptionRow>
  );
}

export function ExperimentalFeaturesCard() {
  const bitcoinAvailable = useBitcoinCompileEnabled();

  // Manifest is preview-only by definition; every desktop entry is a preview
  // feature.
  const previewFeatures = desktopFeatures.filter(
    (feature) => feature.id !== "bitcoin" || bitcoinAvailable,
  );

  return (
    <section className="min-w-0" data-testid="settings-experimental">
      <SettingsSectionHeader
        title="Experiments"
        description={
          <>
            These features are functional but still being refined. Enable them
            to try new capabilities early.
          </>
        }
      />

      <SettingsOptionGroup title="Features">
        {previewFeatures.map((f) => (
          <FeatureRow feature={f} key={f.id} />
        ))}
      </SettingsOptionGroup>
    </section>
  );
}
