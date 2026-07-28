import * as React from "react";
import { Bitcoin, type LucideIcon } from "lucide-react";

import { useBitcoinCompileEnabled } from "@/features/wallet/hooks";
import { SendBitcoinDialog } from "@/features/wallet/ui/SendBitcoinDialog";
import { useFeatureEnabled } from "@/shared/features";
import { cn } from "@/shared/lib/cn";
import { Spinner } from "@/shared/ui/spinner";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/shared/ui/tooltip";

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
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          aria-label={label}
          className={cn(
            "flex h-12 w-12 items-center justify-center rounded-full transition-colors disabled:cursor-not-allowed disabled:opacity-50",
            active
              ? "bg-foreground text-background hover:bg-foreground/90"
              : "bg-muted/60 text-foreground hover:bg-muted/80",
          )}
          data-testid={testId}
          disabled={disabled}
          onClick={onClick}
          type="button"
        >
          {isLoading ? (
            <Spinner aria-hidden="true" className="h-4 w-4 border-2" />
          ) : (
            <Icon className="h-4 w-4" />
          )}
        </button>
      </TooltipTrigger>
      <TooltipContent align="center" side="top">
        {label}
      </TooltipContent>
    </Tooltip>
  );
}
