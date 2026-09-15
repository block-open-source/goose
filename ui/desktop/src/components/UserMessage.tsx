import { memo, useCallback, useEffect, useRef, useState } from 'react';
import ImagePreview from './ImagePreview';
import MarkdownContent from './MarkdownContent';
import {
  getTextAndImageContent,
  imageDataFromMessage,
  type ImageData,
  type Message,
} from '../types/message';
import MessageCopyLink from './MessageCopyLink';
import { formatMessageTimestamp } from '../utils/timeUtils';
import { getTextDirection } from '../utils/textDirection';
import Close from './icons/Close';
import Edit from './icons/Edit';
import { Button } from './ui/button';
import { defineMessages, useIntl } from '../i18n';

const i18n = defineMessages({
  editPlaceholder: {
    id: 'userMessage.editPlaceholder',
    defaultMessage: 'Edit your message...',
  },
  editAriaLabel: {
    id: 'userMessage.editAriaLabel',
    defaultMessage: 'Edit message content',
  },
  emptyError: {
    id: 'userMessage.emptyError',
    defaultMessage: 'Message cannot be empty',
  },
  editInPlaceDescription: {
    id: 'userMessage.editInPlaceDescription',
    defaultMessage:
      '<b>Edit in Place</b> updates this session • <b>Fork Session</b> creates a new session',
  },
  cancel: {
    id: 'userMessage.cancel',
    defaultMessage: 'Cancel',
  },
  cancelAriaLabel: {
    id: 'userMessage.cancelAriaLabel',
    defaultMessage: 'Cancel editing',
  },
  editInPlace: {
    id: 'userMessage.editInPlace',
    defaultMessage: 'Edit in Place',
  },
  editInPlaceAriaLabel: {
    id: 'userMessage.editInPlaceAriaLabel',
    defaultMessage: 'Edit message in place',
  },
  editInPlaceTitle: {
    id: 'userMessage.editInPlaceTitle',
    defaultMessage: 'Update the message in this session',
  },
  forkSession: {
    id: 'userMessage.forkSession',
    defaultMessage: 'Fork Session',
  },
  forkSessionAriaLabel: {
    id: 'userMessage.forkSessionAriaLabel',
    defaultMessage: 'Fork session with edited message',
  },
  forkSessionTitle: {
    id: 'userMessage.forkSessionTitle',
    defaultMessage: 'Create a new session with the edited message',
  },
  editButton: {
    id: 'userMessage.editButton',
    defaultMessage: 'Edit',
  },
  editMessageAriaLabel: {
    id: 'userMessage.editMessageAriaLabel',
    defaultMessage: 'Edit message: {preview}',
  },
  editMessageTitle: {
    id: 'userMessage.editMessageTitle',
    defaultMessage: 'Edit message',
  },
  removeImageFromEdit: {
    id: 'userMessage.removeImageFromEdit',
    defaultMessage: 'Remove image from message',
  },
  editImagesHeading: {
    id: 'userMessage.editImagesHeading',
    defaultMessage: 'Attached images:',
  },
});

interface UserMessageProps {
  message: Message;
  onMessageUpdate?: (
    messageId: string,
    newContent: string,
    editType: 'fork' | 'edit',
    retainedImages: ImageData[]
  ) => void;
}

function UserMessage({ message, onMessageUpdate }: UserMessageProps) {
  const intl = useIntl();
  const contentRef = useRef<HTMLDivElement | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const [isEditing, setIsEditing] = useState(false);
  const [editContent, setEditContent] = useState('');
  const [error, setError] = useState<string | null>(null);

  const { textContent, imagePaths } = getTextAndImageContent(message);
  const timestamp = formatMessageTimestamp(message.created);
  const messageDir = getTextDirection(textContent) ?? undefined;
  const editDir = getTextDirection(editContent) ?? undefined;

  const messageImages: ImageData[] = imageDataFromMessage(message);

  const [removedImageIndices, setRemovedImageIndices] = useState<Set<number>>(new Set());

  useEffect(() => {
    if (!isEditing) {
      setEditContent(textContent);
    }
  }, [message.content, textContent, message.id, isEditing]);

  const initializeEditMode = useCallback(() => {
    setEditContent(textContent);
    setError(null);
    setRemovedImageIndices(new Set());
    window.electron.logInfo(`Entering edit mode with content: ${textContent}`);
  }, [textContent]);

  const handleRemoveImage = useCallback((index: number) => {
    setRemovedImageIndices((prev) => {
      const next = new Set(prev);
      next.add(index);
      return next;
    });
  }, []);

  const handleEditClick = useCallback(() => {
    const newEditingState = !isEditing;
    setIsEditing(newEditingState);

    if (newEditingState) {
      initializeEditMode();
      window.electron.logInfo(`Edit interface shown for message: ${message.id}`);

      setTimeout(() => {
        if (textareaRef.current) {
          textareaRef.current.focus();
          textareaRef.current.setSelectionRange(
            textareaRef.current.value.length,
            textareaRef.current.value.length
          );
        }
      }, 50);
    }

    window.electron.logInfo(`Edit state toggled: ${newEditingState} for message: ${message.id}`);
  }, [isEditing, initializeEditMode, message.id]);

  const handleContentChange = useCallback((e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const newContent = e.target.value;
    setEditContent(newContent);
    setError(null);
    window.electron.logInfo(`Content changed: ${newContent}`);
  }, []);

  const handleSave = useCallback(
    (editType: 'fork' | 'edit') => {
      const retainedImages = messageImages.filter((_, index) => !removedImageIndices.has(index));

      if (editContent.trim().length === 0 && retainedImages.length === 0) {
        setError(intl.formatMessage(i18n.emptyError));
        return;
      }

      setIsEditing(false);

      if (
        editType === 'edit' &&
        editContent.trim() === textContent.trim() &&
        retainedImages.length === messageImages.length
      ) {
        return;
      }

      if (onMessageUpdate && message.id) {
        onMessageUpdate(message.id, editContent, editType, retainedImages);
      }
    },
    [
      editContent,
      textContent,
      onMessageUpdate,
      message.id,
      intl,
      messageImages,
      removedImageIndices,
    ]
  );

  const handleCancel = useCallback(() => {
    window.electron.logInfo('Cancel clicked - reverting to original content');
    setIsEditing(false);
    setEditContent(textContent);
    setError(null);
  }, [textContent]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      window.electron.logInfo(
        `Key pressed: ${e.key}, metaKey: ${e.metaKey}, ctrlKey: ${e.ctrlKey}`
      );

      if (e.key === 'Escape') {
        e.preventDefault();
        handleCancel();
      } else if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        window.electron.logInfo('Cmd+Enter detected, calling handleSave');
        handleSave('fork');
      }
    },
    [handleCancel, handleSave]
  );

  useEffect(() => {
    if (textareaRef.current && isEditing) {
      textareaRef.current.style.height = 'auto';
      textareaRef.current.style.height = `${Math.min(textareaRef.current.scrollHeight, 200)}px`;
    }
  }, [editContent, isEditing]);

  return (
    <div className="w-full mt-[16px] opacity-0 animate-[appear_150ms_ease-in_forwards]">
      <div className="flex flex-col group">
        {isEditing ? (
          <div className="w-full max-w-4xl mx-auto text-text-primary rounded-xl border border-border-primary shadow-lg py-4 px-4 my-2 transition-all duration-200 ease-in-out">
            <textarea
              ref={textareaRef}
              dir={editDir}
              value={editContent}
              onChange={handleContentChange}
              onKeyDown={handleKeyDown}
              className="w-full resize-none bg-transparent text-text-primary placeholder:text-text-secondary border rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-400 focus:border-blue-400 transition-all duration-200 text-base leading-relaxed"
              style={{
                minHeight: '120px',
                maxHeight: '300px',
                padding: '16px',
                fontFamily: 'inherit',
                lineHeight: '1.6',
                wordBreak: 'break-word',
                overflowWrap: 'break-word',
              }}
              placeholder={intl.formatMessage(i18n.editPlaceholder)}
              aria-label={intl.formatMessage(i18n.editAriaLabel)}
              aria-describedby={error ? `error-${message.id}` : undefined}
            />
            {messageImages.length > 0 && (
              <div className="mt-3">
                <p className="text-xs text-text-secondary mb-2">
                  {intl.formatMessage(i18n.editImagesHeading)}
                </p>
                <div className="flex flex-wrap gap-2">
                  {messageImages.map((img, index) => {
                    if (removedImageIndices.has(index)) return null;
                    const dataUrl = `data:${img.mimeType};base64,${img.data}`;
                    return (
                      <div key={index} className="relative group/image">
                        <ImagePreview src={dataUrl} />
                        <button
                          onClick={() => handleRemoveImage(index)}
                          className="absolute -top-1.5 -right-1.5 bg-text-primary text-background-primary rounded-full p-0.5 transition-opacity hover:cursor-pointer focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-400"
                          aria-label={intl.formatMessage(i18n.removeImageFromEdit)}
                        >
                          <Close className="h-3 w-3" />
                        </button>
                      </div>
                    );
                  })}
                </div>
              </div>
            )}
            {error && (
              <div
                id={`error-${message.id}`}
                className="text-red-400 text-xs mt-2 mb-2"
                role="alert"
                aria-live="polite"
              >
                {error}
              </div>
            )}
            <div className="flex justify-between items-center mt-4">
              <div className="text-xs text-text-secondary">
                {intl.formatMessage(i18n.editInPlaceDescription, {
                  b: (chunks: React.ReactNode) => <span className="font-semibold">{chunks}</span>,
                })}
              </div>
              <div className="flex gap-3">
                <Button
                  onClick={handleCancel}
                  variant="ghost"
                  aria-label={intl.formatMessage(i18n.cancelAriaLabel)}
                >
                  {intl.formatMessage(i18n.cancel)}
                </Button>
                <Button
                  onClick={() => handleSave('edit')}
                  variant="secondary"
                  aria-label={intl.formatMessage(i18n.editInPlaceAriaLabel)}
                  title={intl.formatMessage(i18n.editInPlaceTitle)}
                >
                  {intl.formatMessage(i18n.editInPlace)}
                </Button>
                <Button
                  onClick={() => handleSave('fork')}
                  aria-label={intl.formatMessage(i18n.forkSessionAriaLabel)}
                  title={intl.formatMessage(i18n.forkSessionTitle)}
                >
                  {intl.formatMessage(i18n.forkSession)}
                </Button>
              </div>
            </div>
          </div>
        ) : (
          <div className="message flex justify-end w-full">
            <div className="flex-col max-w-[85%] w-fit">
              <div className="flex flex-col group">
                {textContent.trim() && (
                  <div
                    className="user-message-bubble flex bg-text-primary text-background-primary rounded-xl py-2.5 px-4"
                    dir={messageDir}
                  >
                    <div ref={contentRef}>
                      <MarkdownContent
                        content={textContent}
                        className="!text-inherit prose-a:!text-inherit prose-headings:!text-inherit prose-strong:!text-inherit prose-em:!text-inherit prose-li:!text-inherit prose-p:!text-inherit user-message"
                      />
                    </div>
                  </div>
                )}

                {imagePaths.length > 0 && (
                  <div className="flex flex-wrap gap-2 mt-2">
                    {imagePaths.map((imagePath, index) => (
                      <ImagePreview key={index} src={imagePath} />
                    ))}
                  </div>
                )}

                <div className="relative h-[22px] flex justify-end text-right">
                  <div className="absolute w-40 font-mono right-0 text-xs text-text-secondary pt-1 transition-all duration-200 group-hover:-translate-y-4 group-hover:opacity-0">
                    {timestamp}
                  </div>
                  <div className="absolute right-0 pt-1 flex items-center gap-2">
                    <button
                      onClick={handleEditClick}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter' || e.key === ' ') {
                          e.preventDefault();
                          handleEditClick();
                        }
                      }}
                      className="flex items-center gap-1 text-xs text-text-secondary hover:cursor-pointer hover:text-text-primary transition-all duration-200 opacity-0 group-hover:opacity-100 -translate-y-4 group-hover:translate-y-0 focus:outline-none focus:ring-2 focus:ring-blue-400 focus:ring-opacity-50 rounded"
                      aria-label={intl.formatMessage(i18n.editMessageAriaLabel, {
                        preview: `${textContent.substring(0, 50)}${textContent.length > 50 ? '...' : ''}`,
                      })}
                      aria-expanded={isEditing}
                      title={intl.formatMessage(i18n.editMessageTitle)}
                    >
                      <Edit className="h-3 w-3" />
                      <span>{intl.formatMessage(i18n.editButton)}</span>
                    </button>
                    <MessageCopyLink text={textContent} contentRef={contentRef} />
                  </div>
                </div>
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

export default memo(UserMessage);
