/**
 * Custom event names used throughout the application.
 */
export enum AppEvents {
  SESSION_CREATED = 'session-created',
  SESSION_EXTENSIONS_LOADED = 'session-extensions-loaded',
  SESSION_DELETED = 'session-deleted',
  SESSION_RENAMED = 'session-renamed',
  SESSION_FORKED = 'session-forked',
  SESSION_STATUS_UPDATE = 'session-status-update',
  ADD_ACTIVE_SESSION = 'add-active-session',
  CLEAR_INITIAL_MESSAGE = 'clear-initial-message',
  SCROLL_CHAT_TO_BOTTOM = 'scroll-chat-to-bottom',
  HIDE_ALERT_POPOVER = 'hide-alert-popover',
  RESPONSE_STYLE_CHANGED = 'responseStyleChanged',
  BACKEND_SWITCHED = 'backend-switched',
}
