export interface HookEnvelope { version:1; kind:string; capturedAt:string; payload:Record<string,string>; }
export function sanitizeHook(input:Record<string,unknown>,capturedAt?:string):HookEnvelope;
export function enqueue(input:Record<string,unknown>,home?:string):string;
