import * as React from "react";
import { Bitcoin, type LucideIcon } from "lucide-react";

import { useBitcoinCompileEnabled } from "@/features/wallet/hooks";
import { SendBitcoinDialog } from "@/features/wallet/ui/SendBitcoinDialog";
import { useFeatureEnabled } from "@/shared/features";
import { cn } from "@/shared/lib/cn";
import { Spinner } from "@/shared/ui/spinner";

export function ProfileBitcoinQuickAction({
  recipientName,
  recipientPubkey,
}: {
  recipientName: string;
  recipientPubkey: string;
}) {
  const enabled = useFeatureEnabled("bitcoin");
  const compiled = useBitcoinCompileEnabled();
  const [open, setOpen] = React.useState(false);
  if (!enabled || !compiled) return null;

  return (
    <>
      <ProfileQuickAction
        icon={Bitcoin}
        label="Send bitcoin"
        onClick={() => setOpen(true)}
        testId="user-profile-send-bitcoin"
      />
      <SendBitcoinDialog
        onOpenChange={setOpen}
        open={open}
        recipientName={recipientName}
        recipientPubkey={recipientPubkey}
      />
    </>
  );
}

export function ProfileQuickAction({
  active,
  disabled,
  icon: Icon,
  isLoading,
  label,
  onClick,
  testId,
}: {
  active?: boolean;
  disabled?: boolean;
  icon: LucideIcon;
  isLoading?: boolean;
  label: string;
  onClick: () => void;
  testId?: string;
}) {
  return (
    <button
      aria-busy={isLoading || undefined}
      aria-label={label}
      className={cn(
        "flex min-h-20 w-full flex-col items-center justify-center gap-1.5 rounded-xl bg-muted px-2 py-3 text-center transition-colors hover:bg-muted/80 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50",
        active && "bg-foreground text-background hover:bg-foreground/90",
      )}
      data-testid={testId}
      disabled={disabled}
      onClick={onClick}
      type="button"
    >
      {isLoading ? (
        <Spinner aria-hidden="true" className="h-5 w-5 border-2" />
      ) : (
        <Icon
          className={cn("h-5 w-5 text-foreground", active && "text-background")}
        />
      )}
      <span className="min-w-0 text-xs font-medium leading-tight">{label}</span>
    </button>
  );
}
