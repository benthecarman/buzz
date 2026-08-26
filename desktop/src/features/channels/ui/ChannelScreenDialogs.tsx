import type * as React from "react";
import type { useWelcomeAgentCreate } from "@/features/channels/useWelcomeAgentCreate";
import { DeleteMessageConfirmDialog } from "@/features/messages/ui/DeleteMessageConfirmDialog";
import { WelcomeAgentCreateDialog } from "./WelcomeAgentCreateDialog";

type Props = {
  deleteMessageId: string | null;
  guideName: string;
  handleDelete: (message: { id: string }) => Promise<void>;
  setDeleteMessageId: React.Dispatch<React.SetStateAction<string | null>>;
  setEditTargetId: React.Dispatch<React.SetStateAction<string | null>>;
  welcomeAgentCreate: ReturnType<typeof useWelcomeAgentCreate>;
};

export function ChannelScreenDialogs({
  deleteMessageId,
  guideName,
  handleDelete,
  setDeleteMessageId,
  setEditTargetId,
  welcomeAgentCreate,
}: Props) {
  return (
    <>
      <WelcomeAgentCreateDialog
        guideName={guideName}
        isSending={welcomeAgentCreate.isSending}
        onCreateInChat={() => void welcomeAgentCreate.createInChat()}
        onCreateManually={welcomeAgentCreate.createManually}
        onOpenChange={welcomeAgentCreate.setIsOpen}
        open={welcomeAgentCreate.isOpen}
        sendError={welcomeAgentCreate.error}
      />
      <DeleteMessageConfirmDialog
        onConfirm={() => {
          if (deleteMessageId) {
            setEditTargetId(null);
            void handleDelete({ id: deleteMessageId });
          }
          setDeleteMessageId(null);
        }}
        onOpenChange={(open) => {
          if (!open) setDeleteMessageId(null);
        }}
        open={deleteMessageId !== null}
      />
    </>
  );
}
