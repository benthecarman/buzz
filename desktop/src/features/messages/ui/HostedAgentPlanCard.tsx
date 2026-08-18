import { Bot } from "lucide-react";
import * as React from "react";

import type { HostedAgentPlanMessage } from "@/features/messages/lib/hostedAgentZap";
import bitcoinIconUrl from "@/features/profile/assets/bitcoin.svg?inline";
import {
  Attachment,
  AttachmentAction,
  AttachmentActions,
  AttachmentContent,
  AttachmentDescription,
  AttachmentMedia,
  AttachmentTitle,
} from "@/shared/ui/attachment";
import type { MessageZapAction } from "./useMessageZap";

export function HostedAgentPlanCard({
  plan,
  zapAction,
}: {
  plan: HostedAgentPlanMessage;
  zapAction: MessageZapAction;
}) {
  const unavailable = !zapAction.canZap;
  const disabled = unavailable || zapAction.disabled;
  const buttonLabel = unavailable
    ? "Bitcoin unavailable"
    : zapAction.disabled
      ? "Buying…"
      : `Buy agent · ${plan.hourlyPriceSats} sats`;

  const handleBuy = React.useCallback(
    (event: React.MouseEvent<HTMLButtonElement>) => {
      event.preventDefault();
      event.stopPropagation();
      if (!disabled) zapAction.run();
    },
    [disabled, zapAction],
  );

  return (
    <Attachment
      className="mt-1 max-w-xl border-amber-500/30 bg-amber-500/5"
      data-testid="hosted-agent-plan-card"
      size="default"
    >
      <AttachmentMedia aria-hidden="true" className="text-amber-500">
        <Bot />
      </AttachmentMedia>
      <AttachmentContent>
        <AttachmentTitle>{plan.name}</AttachmentTitle>
        <AttachmentDescription className="overflow-visible whitespace-normal text-clip">
          A new isolated agent for {plan.hourlyPriceSats} sats per hour. Data is
          retained for {plan.retentionDays} days after expiry.
        </AttachmentDescription>
      </AttachmentContent>
      <AttachmentActions>
        <AttachmentAction
          aria-label={buttonLabel}
          className="gap-1.5 text-primary-foreground hover:text-primary-foreground disabled:text-primary-foreground"
          data-testid="buy-hosted-agent"
          disabled={disabled}
          onClick={handleBuy}
          size="sm"
          type="button"
          variant="default"
        >
          {!unavailable ? (
            <img
              alt=""
              aria-hidden="true"
              className="h-4 w-4"
              src={bitcoinIconUrl}
            />
          ) : null}
          {buttonLabel}
        </AttachmentAction>
      </AttachmentActions>
    </Attachment>
  );
}
