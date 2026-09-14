'use client';

import * as React from 'react';
import * as DialogPrimitive from '@radix-ui/react-dialog';
import { XIcon } from 'lucide-react';

import { cn } from '../../utils';
import { defineMessages, useIntl } from '../../i18n';
import { scheduleBodyPointerEventsRelease } from '../../utils/radixPointerEvents';

const i18n = defineMessages({
  close: {
    id: 'dialog.close',
    defaultMessage: 'Close',
  },
});

function Dialog({ ...props }: React.ComponentProps<typeof DialogPrimitive.Root>) {
  return <DialogPrimitive.Root data-slot="dialog" {...props} />;
}

function DialogTrigger({ ...props }: React.ComponentProps<typeof DialogPrimitive.Trigger>) {
  return <DialogPrimitive.Trigger data-slot="dialog-trigger" {...props} />;
}

function DialogPortal({ ...props }: React.ComponentProps<typeof DialogPrimitive.Portal>) {
  return <DialogPrimitive.Portal data-slot="dialog-portal" {...props} />;
}

const DialogOverlay = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Overlay>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Overlay>
>(({ className, ...props }, ref) => (
  <DialogPrimitive.Overlay
    ref={ref}
    data-slot="dialog-overlay"
    className={cn(
      'data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 fixed inset-0 z-40 bg-black/50',
      className
    )}
    {...props}
  />
));
DialogOverlay.displayName = DialogPrimitive.Overlay.displayName;

/**
 * Renders nothing. Place it inside `DialogPrimitive.Content` so that it unmounts together
 * with the content's dismissable layer, and schedules the body pointer-events recovery
 * from there.
 *
 * Two copies of Radix's dismissable layer are installed, so a dialog opened from a
 * dropdown menu can leave `pointer-events: none` on the body and freeze every click in
 * the window - see `releaseStuckBodyPointerEvents`. The recovery has to run after the
 * content's layer has done its own restore, which is what keeps writing the stale value
 * back. Hanging it off the overlay is not enough: overlay and content have separate
 * presence lifecycles and exit animations (the overlay's default 150ms against the
 * content's `duration-200`), so the overlay unmounts first and a check scheduled from
 * there runs while the closing content is still mounted, only for the content's later
 * cleanup to re-apply `none` with nothing left to clear it. `DialogContent` itself is no
 * home for the effect either: it stays mounted for the life of the `Dialog`, and only the
 * portal's children unmount on close.
 */
function DialogPointerEventsRecovery() {
  React.useEffect(() => () => scheduleBodyPointerEventsRelease(), []);
  return null;
}

function DialogContent({
  className,
  children,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Content>) {
  const intl = useIntl();
  return (
    <DialogPortal data-slot="dialog-portal">
      <DialogOverlay />
      <DialogPrimitive.Content
        data-slot="dialog-content"
        className={cn(
          'bg-background-primary data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95 fixed top-[50%] left-[50%] z-50 grid w-full max-w-[calc(100%-2rem)] translate-x-[-50%] translate-y-[-50%] gap-4 rounded-lg border p-6 shadow-lg duration-200 sm:max-w-lg',
          className
        )}
        {...props}
      >
        <DialogPointerEventsRecovery />
        {children}
        <DialogPrimitive.Close className="ring-offset-background p-1 hover:bg-background-secondary rounded-full focus:ring-ring data-[state=open]:bg-background-secondary transition-all duration-200 data-[state=open]:text-text-secondary absolute top-4 right-4 opacity-70 hover:opacity-100 focus:ring-2 focus:ring-offset-2 focus:outline-hidden disabled:pointer-events-none [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4">
          <XIcon />
          <span className="sr-only">{intl.formatMessage(i18n.close)}</span>
        </DialogPrimitive.Close>
      </DialogPrimitive.Content>
    </DialogPortal>
  );
}

function DialogHeader({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div
      data-slot="dialog-header"
      className={cn('flex flex-col gap-1 mb-2 text-center sm:text-left', className)}
      {...props}
    />
  );
}

function DialogFooter({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div
      data-slot="dialog-footer"
      className={cn('flex flex-col-reverse gap-2 sm:flex-row sm:justify-end', className)}
      {...props}
    />
  );
}

function DialogTitle({ className, ...props }: React.ComponentProps<typeof DialogPrimitive.Title>) {
  return (
    <DialogPrimitive.Title
      data-slot="dialog-title"
      className={cn('text-lg leading-none font-medium', className)}
      {...props}
    />
  );
}

function DialogDescription({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Description>) {
  return (
    <DialogPrimitive.Description
      data-slot="dialog-description"
      className={cn('text-text-secondary text-sm', className)}
      {...props}
    />
  );
}

export {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogOverlay,
  DialogPointerEventsRecovery,
  DialogPortal,
  DialogTitle,
  DialogTrigger,
};
