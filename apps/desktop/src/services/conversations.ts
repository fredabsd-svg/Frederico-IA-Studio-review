/**
 * Operações de conversas e mensagens. Camada `services/`.
 */

import { dispatch } from "./api";
import type {
  ConversationView,
  ConversationWithMessages,
  MessageEventView,
  MessageSendResult,
  MessageView,
} from "./contracts";

export function listConversations(): Promise<ConversationView[]> {
  return dispatch<ConversationView[]>({ kind: "conversation_list" });
}

export function getConversation(
  id: string,
): Promise<ConversationWithMessages> {
  return dispatch<ConversationWithMessages>({ kind: "conversation_get", id });
}

export function createConversation(
  provider: string,
  model: string,
  title: string | null,
): Promise<ConversationView> {
  return dispatch<ConversationView>({
    kind: "conversation_create",
    provider,
    model,
    title,
  });
}

export function renameConversation(
  id: string,
  title: string | null,
): Promise<null> {
  return dispatch<null>({ kind: "conversation_rename", id, title });
}

export function setConversationModel(
  id: string,
  provider: string,
  model: string,
): Promise<null> {
  return dispatch<null>({
    kind: "conversation_set_model",
    id,
    provider,
    model,
  });
}

export function deleteConversation(id: string): Promise<null> {
  return dispatch<null>({ kind: "conversation_delete", id });
}

export function sendMessage(
  conversationId: string,
  content: string,
): Promise<MessageSendResult> {
  return dispatch<MessageSendResult>({
    kind: "message_send",
    conversation_id: conversationId,
    content,
  });
}

export function getRunEvents(
  messageId: string,
  sinceSeq: number,
): Promise<MessageEventView[]> {
  return dispatch<MessageEventView[]>({
    kind: "run_get_events",
    message_id: messageId,
    since_seq: sinceSeq,
  });
}

export function cancelRun(runId: string): Promise<null> {
  return dispatch<null>({ kind: "run_cancel", run_id: runId });
}

// Helper de tipo para extrair mensagens de assistant.
export function isAssistantMessage(m: MessageView): boolean {
  return m.role === "assistant";
}

export function isUserMessage(m: MessageView): boolean {
  return m.role === "user";
}
