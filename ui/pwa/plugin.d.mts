import type { Plugin } from "vite";

export function shellList(bundleFiles: string[], statics: string[]): string[];
export function shellVersion(shell: string[]): string;
export function buildServiceWorker(shell: string[]): string;
export function pwaFiles(): Plugin;
