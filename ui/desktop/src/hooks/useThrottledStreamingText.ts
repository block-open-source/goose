import { useEffect, useRef, useState } from 'react';

export const STREAMING_RENDER_COOLDOWN_MS = 50;

export function useThrottledStreamingText(
  content: string,
  enabled: boolean,
  cooldownMs = STREAMING_RENDER_COOLDOWN_MS
): string {
  const [renderedText, setRenderedText] = useState(content);
  const renderedTextRef = useRef(content);
  const latestTextRef = useRef(content);
  const cooldownTimerRef = useRef<number | null>(null);

  latestTextRef.current = content;

  useEffect(() => {
    if (!enabled) {
      if (cooldownTimerRef.current !== null) {
        window.clearTimeout(cooldownTimerRef.current);
        cooldownTimerRef.current = null;
      }
      renderedTextRef.current = content;
      setRenderedText(content);
      return;
    }

    if (cooldownTimerRef.current === null && content !== renderedTextRef.current) {
      renderedTextRef.current = content;
      setRenderedText(content);
    }
  }, [content, enabled]);

  useEffect(() => {
    if (!enabled) return;

    cooldownTimerRef.current = window.setTimeout(() => {
      cooldownTimerRef.current = null;
      const latestText = latestTextRef.current;
      if (latestText !== renderedTextRef.current) {
        renderedTextRef.current = latestText;
        setRenderedText(latestText);
      }
    }, cooldownMs);

    return () => {
      if (cooldownTimerRef.current !== null) {
        window.clearTimeout(cooldownTimerRef.current);
        cooldownTimerRef.current = null;
      }
    };
  }, [cooldownMs, enabled, renderedText]);

  return enabled ? renderedText : content;
}
