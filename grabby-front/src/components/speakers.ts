export interface SpeakerEntry {
  original: string; // e.g. "Speaker A"
  current: string;  // edited name
}

/** Extract original and renamed speaker labels from transcript text. */
export function extractSpeakers(text: string): SpeakerEntry[] {
  const regex = /^([^:\r\n]+):/gm;
  const seen = new Set<string>();
  let match: RegExpExecArray | null;
  while ((match = regex.exec(text)) !== null) {
    seen.add(match[1]);
  }
  return Array.from(seen)
    .sort()
    .map((s) => ({ original: s, current: s }));
}

/** Replace all occurrences of a speaker label in the text. */
export function replaceSpeaker(text: string, oldName: string, newName: string): string {
  // Replace "OldName:" at start of line with "NewName:"
  const escaped = oldName.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return text.replace(new RegExp(`^${escaped}:`, "gm"), () => `${newName}:`);
}

