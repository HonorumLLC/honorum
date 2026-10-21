export interface Meeting { days: string; start: number; end: number; location?: string }
export interface Exam { date: string; start: number; end: number; location?: string }
export interface Term { id: string; name: string; start?: string; end?: string; timezone?: string; structure_snapshot?: string; seats_snapshot?: string; descriptions_snapshot?: string }
export interface Section { id: string; course: string; subject: string; title: string; num: string; type: string; link?: string; instructors: string[]; meetings: Meeting[]; exams?: Exam[]; available: number; fetched_at?: number; capacity?: number; waitlist?: number; start?: string; end?: string; credits?: number; mode?: string; attributes?: string[] }
export interface Course { code: string; subject: string; title: string; sections: Section[] }
export interface Filters { open: boolean; days: string; time: string; mode: string; level: string; attr: string }
export type CatalogQuery = { type: 'search'; q: string; f: Filters; limit: number; offset: number } | { type: 'sections'; ids: string[] } | { type: 'course'; code: string } | { type: 'courses'; codes: string[] };
export type CatalogRequest = (CatalogQuery & { id: number; key: string }) | { type: 'load'; inst: string; term: string; structureHash: string; seatsHash: string; requestHash?: string };
export interface CatalogReply { id?: number; error?: string; courses?: Course[]; sections?: Section[]; course?: Course | null; type?: string; key?: string; [key: string]: unknown }
