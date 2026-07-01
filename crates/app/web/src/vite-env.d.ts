/// <reference types="vite/client" />

declare global {
  interface Window {
    marked: {
      setOptions(opts: Record<string, unknown>): void;
      parse(src: string): string;
    };
    hljs: {
      highlightElement(el: HTMLElement): void;
    };
    __synapse?: Record<string, unknown>;
    __SYNAPSE__?: Record<string, unknown>;
    __synapseHaptic__?: (style: string) => void;
    __synapseCopy__?: (text: string) => void;
    webkit?: {
      messageHandlers?: {
        synapse?: { postMessage(msg: unknown): void };
      };
    };
  }
}

export {};
