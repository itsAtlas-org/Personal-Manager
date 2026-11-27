// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Who did you meet?" (spec 6.3): one card per meeting, newest first, in a dialog. Saving or skipping
// a card takes it off the list; answering the last one closes the dialog. "Skip all" skips every
// card still showing, one answer each, and stops at the first one that fails (the cards that went
// stay gone). It reads nothing itself: the meetings come in, every answer goes out through ipc, so
// it opens in the main window, over the Calendar tab and in the pop-out Calendar alike.

import { useEffect, useRef, useState } from "react";
import { answerMeeting } from "../../lib/ipc";
import { meetingKey } from "../../lib/meetings";
import { errorWords } from "../../lib/people";
import type { OwedMeeting } from "../../lib/types";
import { Button, Callout, Dialog } from "../ui";
import { MeetCard } from "./MeetCard";

interface Props {
  /** The meetings to show, newest first. Taken once when the dialog opens. */
  meetings: OwedMeeting[];
  onClose: () => void;
  /** Something was answered: the owner re-reads what is owed. */
  onAnswered?: () => void;
  onOpenPerson?: (uid: string) => void;
  /** The dialog's name: "Log who you met" when it was asked for on one event. */
  title?: string;
}

export function MeetingDialog({
  meetings,
  onClose,
  onAnswered,
  onOpenPerson,
  title = "Who did you meet?",
}: Props) {
  const [left, setLeft] = useState<OwedMeeting[]>(meetings);
  const [skipping, setSkipping] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  // Cards with an answer in flight: Skip all and Close wait for them, so a skip never runs over a
  // Save and the dialog never closes under one.
  const [inFlight, setInFlight] = useState(0);
  const answers = useRef(0);
  const cards = useRef<HTMLDivElement>(null);

  // Saving or skipping the last card closes the dialog. Decided on the list itself, so two answers
  // landing close together can't each see a list the other has already shortened.
  useEffect(() => {
    if (answers.current > 0 && left.length === 0) onClose();
    // The card that was answered took the focus with it: put it on the cards that remain.
    else if (answers.current > 0) cards.current?.focus();
    // `onClose` is the owner's and may change every render; only the list closing it matters.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [left]);

  const drop = (keys: ReadonlySet<string>) =>
    setLeft((prev) => prev.filter((m) => !keys.has(meetingKey(m))));

  function gone(done: OwedMeeting) {
    answers.current += 1;
    onAnswered?.();
    drop(new Set([meetingKey(done)]));
  }

  async function skipAll() {
    setSkipping(true);
    setProblem(null);
    const skipped = new Set<string>();
    try {
      for (const m of left) {
        // A meeting with no iCal id can only be logged, so it stays.
        if (m.uid === "") continue;
        await answerMeeting(m.uid, m.start, [], "skipped", null, m.title);
        skipped.add(meetingKey(m));
      }
    } catch (err) {
      setProblem(errorWords(err));
    } finally {
      setSkipping(false);
      if (skipped.size > 0) {
        answers.current += 1;
        onAnswered?.();
        drop(skipped);
      }
    }
  }

  const many = left.length > 1;
  return (
    <Dialog
      open
      onClose={onClose}
      title={title}
      subtitle={
        many
          ? `${left.length} meetings are waiting for an answer.`
          : "Tick who was there, or skip it."
      }
      helpId="people-meet"
      className="max-w-xl"
      footer={
        <>
          {many && (
            <Button
              variant="tertiary"
              disabled={skipping || inFlight > 0}
              onClick={() => void skipAll()}
            >
              Skip all
            </Button>
          )}
          <Button variant="secondary" disabled={skipping || inFlight > 0} onClick={onClose}>
            Close
          </Button>
        </>
      }
    >
      {problem && (
        <Callout tone="danger" size="sm" className="mt-3">
          {problem}
        </Callout>
      )}
      <div
        ref={cards}
        tabIndex={-1}
        role="group"
        aria-label="Meetings to answer"
        className="mt-4 flex max-h-[60vh] flex-col gap-3 overflow-y-auto pr-1 outline-none"
      >
        {left.map((m) => (
          <MeetCard
            key={meetingKey(m)}
            meeting={m}
            locked={skipping}
            onAnswered={gone}
            onBusy={(busy) => setInFlight((n) => n + (busy ? 1 : -1))}
            onOpenPerson={onOpenPerson}
          />
        ))}
      </div>
    </Dialog>
  );
}
