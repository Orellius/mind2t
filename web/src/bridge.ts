// The panel's half of the bridge: one place that talks to Swift, one place that is
// talked to. Nothing else in the panel touches `window.webkit`.
//
// Installed at module import, before React renders. That ordering is deliberate -- the
// host may probe liveness (`ping`) the instant the document finishes loading, and a
// receiver that only exists after the first render would make the probe a race rather
// than a proof.

import { decodeHostMessage, type HostMessage, type PanelMessage } from "./protocol";

type Subscriber = (message: HostMessage) => void;

interface WebKitBridge {
  readonly messageHandlers?: {
    readonly ruuah?: { postMessage(body: unknown): void };
  };
}

/** Messages that arrive before a subscriber exists, replayed in order once one does. */
const pending: HostMessage[] = [];
let subscriber: Subscriber | null = null;

function handler(): { postMessage(body: unknown): void } | null {
  const webkit = (window as { webkit?: WebKitBridge }).webkit;
  return webkit?.messageHandlers?.ruuah ?? null;
}

/**
 * Sends one message to the host.
 *
 * Returns whether a handler existed to take it. A missing handler is the dead-bridge
 * case and it is reported rather than swallowed: WKWebView does not throw when a script
 * message handler was never registered, so an unchecked postMessage is indistinguishable
 * from a working one.
 */
export function send(message: PanelMessage): boolean {
  const target = handler();
  if (target === null) {
    console.error("[ruuah] no host message handler; the bridge is not installed");
    return false;
  }
  target.postMessage(message);
  return true;
}

export function subscribe(next: Subscriber): () => void {
  subscriber = next;
  while (pending.length > 0) {
    const message = pending.shift();
    if (message !== undefined) next(message);
  }
  return () => {
    if (subscriber === next) subscriber = null;
  };
}

function receive(raw: unknown): void {
  const decoded = decodeHostMessage(raw);
  if (!decoded.ok) {
    // Reported to the host, not merely logged: the console of a panel nobody has a
    // debugger attached to is the same as silence.
    send({ kind: "decodeError", detail: decoded.error });
    return;
  }
  const message = decoded.value;
  if (message.kind === "ping") {
    // Answered here rather than in the UI, so liveness measures the BRIDGE and not
    // whatever the panel happens to be rendering.
    send({ kind: "pong", nonce: message.nonce });
    return;
  }
  if (subscriber === null) {
    pending.push(message);
    return;
  }
  subscriber(message);
}

declare global {
  interface Window {
    /** The host's single entry point into this document. Called via evaluateJavaScript. */
    __ruuahReceive?: (raw: unknown) => void;
  }
}

window.__ruuahReceive = receive;
