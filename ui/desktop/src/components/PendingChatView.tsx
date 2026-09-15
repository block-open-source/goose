import { ChatState } from '../types/chatState';
import type { UserInput } from '../types/message';
import ImagePreview from './ImagePreview';
import { MainPanelLayout } from './Layout/MainPanelLayout';
import LoadingGoose from './LoadingGoose';
import MarkdownContent from './MarkdownContent';

export default function PendingChatView({ initialMessage }: { initialMessage?: UserInput }) {
  const imageSrcs =
    initialMessage?.images.map((image) => `data:${image.mimeType};base64,${image.data}`) ?? [];
  const hasText = Boolean(initialMessage?.msg.trim());
  const hasContent = hasText || imageSrcs.length > 0;

  return (
    <div className="h-full flex flex-col min-h-0" data-testid="pending-chat">
      <MainPanelLayout backgroundColor="bg-background-primary" removeTopPadding>
        <div className="flex flex-col flex-1 min-h-0 relative">
          <div className="flex-1 min-h-0 overflow-auto px-6 pt-12 pb-10">
            {hasContent ? (
              <div className="message flex justify-end w-full mt-4">
                <div className="flex-col max-w-[85%] w-fit">
                  {hasText ? (
                    <div className="user-message-bubble flex bg-text-primary text-background-primary rounded-xl py-2.5 px-4">
                      <MarkdownContent
                        content={initialMessage!.msg}
                        className="!text-inherit prose-a:!text-inherit prose-headings:!text-inherit prose-strong:!text-inherit prose-em:!text-inherit prose-li:!text-inherit prose-p:!text-inherit user-message"
                      />
                    </div>
                  ) : null}
                  {imageSrcs.length > 0 ? (
                    <div className="flex flex-wrap gap-2 mt-2">
                      {imageSrcs.map((src, index) => (
                        <ImagePreview key={index} src={src} />
                      ))}
                    </div>
                  ) : null}
                </div>
              </div>
            ) : null}
          </div>
          <div className="absolute bottom-1 left-4 z-20 pointer-events-none">
            <LoadingGoose chatState={ChatState.LoadingConversation} />
          </div>
        </div>
      </MainPanelLayout>
    </div>
  );
}
