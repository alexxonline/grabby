import assert from "node:assert/strict";
import test from "node:test";
import { extractSpeakers, replaceSpeaker } from "../src/components/speakers.ts";

test("renamed speakers remain editable after loading saved text", () => {
  const original = "Speaker 1: Hello\nSpeaker 2: Hi\nSpeaker 1: Goodbye";
  const saved = replaceSpeaker(original, "Speaker 1", "Alice");
  assert.deepEqual(extractSpeakers(saved).map(s => s.current), ["Alice", "Speaker 2"]);
  const editedAgain = replaceSpeaker(saved, "Alice", "José Smith");
  assert.equal(editedAgain, "José Smith: Hello\nSpeaker 2: Hi\nJosé Smith: Goodbye");
  assert.deepEqual(extractSpeakers(editedAgain).map(s => s.current), ["José Smith", "Speaker 2"]);
});

test("speaker rename preserves literal dollar signs and regex characters", () => {
  const saved = replaceSpeaker("Speaker 1: Hello", "Speaker 1", "Alice $& [QA]");
  assert.equal(saved, "Alice $& [QA]: Hello");
  assert.equal(replaceSpeaker(saved, "Alice $& [QA]", "Bob"), "Bob: Hello");
});

test("speaker extraction does not span blank lines or unlabelled text", () => {
  assert.deepEqual(extractSpeakers("Unlabelled text\n\nAlice: Hi\r\nAlice: Again"), [{original: "Alice", current: "Alice"}]);
});
