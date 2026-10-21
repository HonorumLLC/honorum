/// <reference lib="webworker" />
import { get, set } from './store.js';
import { createCatalogService } from './catalogService.js';
import type { CatalogRequest } from './domain';
const scope = self as unknown as DedicatedWorkerGlobalScope;
const handle = createCatalogService({ post: (message: unknown) => scope.postMessage(message), read: get, write: set });
scope.onmessage = (event: MessageEvent<CatalogRequest>) => { void handle(event.data); };
