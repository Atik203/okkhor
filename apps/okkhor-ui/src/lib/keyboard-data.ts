/**
 * Static on-screen keyboard data (0.5 Task 5).
 *
 * The grid below is a Unicode Bangla character table curated for this UI: it
 * inserts Unicode text through `text.insert` and does NOT replay any layout's
 * roman keystrokes. Per-layout roman mappings are intentionally not bundled
 * (no invented mappings). The layout list mirrors the daemon's
 * `layouts.list`; the active layout only names which daemon layout handles
 * hardware typing.
 *
 * Privacy: `localStorage` holds only non-sensitive UI state (group order,
 * pinned single-character favorites) under versioned keys. Recents are
 * session-only in-memory state and are never persisted.
 */

export interface KeyDef {
  /** Unicode text inserted on click (base layer). */
  base: string;
  /** Optional Shift-layer text. Absent means Shift shows `base`. */
  shift?: string;
}

export interface KeyGroup {
  id: string;
  label: string;
  keys: KeyDef[];
}

export const KEY_GROUPS: KeyGroup[] = [
  {
    id: "vowels",
    label: "Vowels",
    keys: ["অ", "আ", "ই", "ঈ", "উ", "ঊ", "ঋ", "এ", "ঐ", "ও", "ঔ"].map((base) => ({
      base,
    })),
  },
  {
    id: "consonants",
    label: "Consonants",
    keys: [
      "ক", "খ", "গ", "ঘ", "ঙ",
      "চ", "ছ", "জ", "ঝ", "ঞ",
      "ট", "ঠ", "ড", "ঢ", "ণ",
      "ত", "থ", "দ", "ধ", "ন",
      "প", "ফ", "ব", "ভ", "ম",
      "য", "র", "ল", "শ", "ষ",
      "স", "হ", "ড়", "ঢ়", "য়", "ৎ",
    ].map((base) => ({ base })),
  },
  {
    id: "signs",
    label: "Vowel signs and marks",
    keys: ["া", "ি", "ী", "ু", "ূ", "ৃ", "ে", "ৈ", "ো", "ৌ", "্", "ং", "ঃ", "ঁ"].map(
      (base) => ({ base }),
    ),
  },
  {
    id: "digits",
    label: "Digits",
    // Shift layer offers the ASCII digit for quick entry alongside the
    // Bangla digit. This pairing is this table's own definition, not a
    // claim about any hardware layout's Shift behavior.
    keys: ["০", "১", "২", "৩", "৪", "৫", "৬", "৭", "৮", "৯"].map((base, index) => ({
      base,
      shift: String(index),
    })),
  },
  {
    id: "symbols",
    label: "Punctuation and symbols",
    keys: [
      { base: "।", shift: "॥" },
      { base: ",", shift: ";" },
      { base: ".", shift: ":" },
      { base: "?", shift: "!" },
      { base: "-", shift: "–" },
      { base: "'", shift: '"' },
      { base: "(", shift: ")" },
      { base: "%", shift: "৳" },
      { base: "+", shift: "=" },
      { base: "/", shift: "@" },
      { base: "#", shift: "*" },
    ],
  },
  {
    id: "conjuncts",
    label: "Common conjuncts",
    keys: ["ক্ষ", "জ্ঞ", "ত্র", "শ্র", "দ্ধ", "ন্ত", "স্ত", "ন্দ", "ম্প", "ঙ্ক", "ঞ্চ", "স্ফ"].map(
      (base) => ({ base }),
    ),
  },
];

/** Canonical group order; also the fallback when stored state is invalid. */
export const DEFAULT_GROUP_ORDER: string[] = KEY_GROUPS.map((group) => group.id);

/** Versioned, non-sensitive `localStorage` keys (UI state only, no text logs). */
export const OSK_STORAGE_VERSION = 1;
export const GROUP_ORDER_KEY = `okkhor.osk.v${OSK_STORAGE_VERSION}.groupOrder`;
export const FAVORITES_KEY = `okkhor.osk.v${OSK_STORAGE_VERSION}.favorites`;

/** Maximum pinned favorites (single characters, user preference — not a log). */
export const MAX_FAVORITES = 24;
/** Session-only recents cap (in-memory only, never persisted). */
export const MAX_RECENTS = 12;

/** Visible text for a key on the active Shift layer. */
export function resolveKeyText(key: KeyDef, shift: boolean): string {
  if (shift && key.shift !== undefined) {
    return key.shift;
  }
  return key.base;
}

function readStoredList(key: string): string[] | null {
  try {
    const raw = window.localStorage.getItem(key);
    if (raw === null) {
      return null;
    }
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) {
      return null;
    }
    return parsed.filter((item): item is string => typeof item === "string");
  } catch {
    return null;
  }
}

/** Load the persisted group order, sanitized: known ids, no dupes, all present. */
export function loadGroupOrder(): string[] {
  const stored = readStoredList(GROUP_ORDER_KEY);
  if (stored === null) {
    return [...DEFAULT_GROUP_ORDER];
  }
  const known = new Set(DEFAULT_GROUP_ORDER);
  const seen = new Set<string>();
  const order = stored.filter((id) => known.has(id) && !seen.has(id) && (seen.add(id), true));
  for (const id of DEFAULT_GROUP_ORDER) {
    if (!seen.has(id)) {
      order.push(id);
    }
  }
  return order;
}

/** Persist a group order; failures (private mode, quota) are silent. */
export function saveGroupOrder(order: string[]): void {
  try {
    window.localStorage.setItem(GROUP_ORDER_KEY, JSON.stringify(order));
  } catch {
    // Non-fatal: the grid keeps working with in-memory order.
  }
}

/** Move a group one step; out-of-range moves are no-ops. Pure (testable). */
export function moveGroup(order: string[], id: string, direction: -1 | 1): string[] {
  const index = order.indexOf(id);
  const target = index + direction;
  if (index < 0 || target < 0 || target >= order.length) {
    return order;
  }
  const next = [...order];
  const [removed] = next.splice(index, 1);
  next.splice(target, 0, removed);
  return next;
}

/** Load pinned favorites (single characters chosen by the user). */
export function loadFavorites(): string[] {
  const stored = readStoredList(FAVORITES_KEY);
  if (stored === null) {
    return [];
  }
  const seen = new Set<string>();
  return stored
    .filter((key) => key.length > 0 && key.length <= 8 && !seen.has(key) && (seen.add(key), true))
    .slice(0, MAX_FAVORITES);
}

/** Persist favorites; failures are silent. */
export function saveFavorites(favorites: string[]): void {
  try {
    window.localStorage.setItem(FAVORITES_KEY, JSON.stringify(favorites.slice(0, MAX_FAVORITES)));
  } catch {
    // Non-fatal.
  }
}

/** Toggle a key in the favorites list (pure). */
export function toggleFavorite(favorites: string[], key: string): string[] {
  if (favorites.includes(key)) {
    return favorites.filter((item) => item !== key);
  }
  return [...favorites, key].slice(0, MAX_FAVORITES);
}

/**
 * Prepend a key to the session-only recents (pure, in-memory only).
 * Never touches `localStorage`: recents must not persist keystrokes.
 */
export function pushRecent(recents: string[], key: string, max: number = MAX_RECENTS): string[] {
  return [key, ...recents.filter((item) => item !== key)].slice(0, max);
}

/** Fallback layout list when the daemon is unreachable (codes only). */
export const FALLBACK_LAYOUTS: { code: string; name: string }[] = [
  { code: "avro", name: "Avro" },
  { code: "khipro", name: "Khipro" },
  { code: "phonetic", name: "Phonetic" },
  { code: "national", name: "Bangla (National)" },
  { code: "bijoy", name: "Bijoy" },
  { code: "probhat", name: "Probhat" },
];

export interface EmojiEntry {
  ch: string;
  /** Space-separated English keywords. */
  en: string;
  /** Space-separated Bengali keywords. */
  bn: string;
}

/**
 * Small bundled emoji list (~120 entries, no network). Each entry carries
 * English and Bengali keywords for search; click-to-insert sends the single
 * character through `text.insert`.
 */
export const EMOJI: EmojiEntry[] = [
  { ch: "😀", en: "grin smile happy", bn: "হাসি খুশি" },
  { ch: "😁", en: "smile grin teeth happy", bn: "হাসি খুশি" },
  { ch: "😂", en: "laugh tears joy lol", bn: "হাসি কান্না মজা" },
  { ch: "🤣", en: "laugh rolling lol", bn: "হাসি মজা" },
  { ch: "😊", en: "smile blush happy", bn: "হাসি খুশি" },
  { ch: "😍", en: "love heart eyes", bn: "ভালোবাসা" },
  { ch: "😘", en: "kiss love", bn: "চুমু ভালোবাসা" },
  { ch: "🥰", en: "affection love smile", bn: "ভালোবাসা" },
  { ch: "😎", en: "cool sunglasses", bn: "ভাব" },
  { ch: "🤔", en: "think thought", bn: "চিন্তা ভাবনা" },
  { ch: "😐", en: "neutral straight face", bn: "গম্ভীর" },
  { ch: "🙁", en: "frown sad", bn: "দুঃখ মনখারাপ" },
  { ch: "😢", en: "cry tear sad", bn: "কান্না দুঃখ" },
  { ch: "😭", en: "sob cry tears", bn: "কান্না" },
  { ch: "😡", en: "angry rage red", bn: "রাগ" },
  { ch: "🥳", en: "party celebrate birthday", bn: "উৎসব জন্মদিন" },
  { ch: "😴", en: "sleep snore tired", bn: "ঘুম" },
  { ch: "🤯", en: "mind blown shocked", bn: "অবাক" },
  { ch: "😷", en: "mask sick ill", bn: "অসুস্থ মাস্ক" },
  { ch: "🤒", en: "sick fever ill", bn: "অসুস্থ জ্বর" },
  { ch: "🤕", en: "hurt bandage injury", bn: "ব্যথা আঘাত" },
  { ch: "😇", en: "angel halo innocent", bn: "ফেরেশতা" },
  { ch: "🙄", en: "roll eyes annoyed", bn: "বিরক্ত" },
  { ch: "😬", en: "grimace nervous", bn: "অস্বস্তি" },
  { ch: "👍", en: "thumbs up like yes good", bn: "ভালো ঠিক" },
  { ch: "👎", en: "thumbs down dislike no bad", bn: "খারাপ" },
  { ch: "👏", en: "clap applause praise", bn: "হাততালি" },
  { ch: "🙏", en: "pray thanks please fold", bn: "প্রার্থনা ধন্যবাদ দোয়া" },
  { ch: "💪", en: "muscle strong power", bn: "শক্তি" },
  { ch: "✋", en: "hand stop high five", bn: "হাত থামো" },
  { ch: "👋", en: "wave hello bye", bn: "হ্যালো বিদায়" },
  { ch: "🤝", en: "handshake deal agree", bn: "চুক্তি" },
  { ch: "✌️", en: "victory peace two", bn: "জয় শান্তি" },
  { ch: "👌", en: "ok perfect fine", bn: "ঠিক" },
  { ch: "🤲", en: "palms dua pray together", bn: "দোয়া প্রার্থনা" },
  { ch: "🫶", en: "heart hands love", bn: "ভালোবাসা" },
  { ch: "❤️", en: "heart red love", bn: "ভালোবাসা হৃদয়" },
  { ch: "💔", en: "broken heart sad", bn: "ভাঙা মন দুঃখ" },
  { ch: "💖", en: "sparkling heart love", bn: "ভালোবাসা" },
  { ch: "💯", en: "hundred perfect score", bn: "শত" },
  { ch: "⭐", en: "star favorite", bn: "তারা" },
  { ch: "🌟", en: "glowing star sparkle", bn: "তারা" },
  { ch: "✨", en: "sparkle shine magic", bn: "ঝিকমিক" },
  { ch: "🔥", en: "fire hot lit", bn: "আগুন" },
  { ch: "💧", en: "drop water tear", bn: "পানি ফোঁটা" },
  { ch: "⚡", en: "lightning fast bolt", bn: "বিদ্যুৎ বজ্র" },
  { ch: "✅", en: "check tick done yes", bn: "ঠিক সম্পন্ন" },
  { ch: "❌", en: "cross wrong no x", bn: "ভুল" },
  { ch: "❓", en: "question help what", bn: "প্রশ্ন" },
  { ch: "❗", en: "exclaim alert important", bn: "বিস্ময় জরুরি" },
  { ch: "🐱", en: "cat kitten", bn: "বিড়াল" },
  { ch: "🐶", en: "dog puppy", bn: "কুকুর" },
  { ch: "🐦", en: "bird sparrow", bn: "পাখি" },
  { ch: "🐟", en: "fish", bn: "মাছ" },
  { ch: "🐘", en: "elephant", bn: "হাতি" },
  { ch: "🦁", en: "lion king", bn: "সিংহ" },
  { ch: "🐯", en: "tiger", bn: "বাঘ" },
  { ch: "🐵", en: "monkey", bn: "বানর" },
  { ch: "🐄", en: "cow", bn: "গরু" },
  { ch: "🐐", en: "goat", bn: "ছাগল" },
  { ch: "🐓", en: "rooster chicken hen", bn: "মোরগ মুরগি" },
  { ch: "🦆", en: "duck", bn: "হাঁস" },
  { ch: "🐝", en: "bee honey", bn: "মৌমাছি মধু" },
  { ch: "🦋", en: "butterfly", bn: "প্রজাপতি" },
  { ch: "🐜", en: "ant", bn: "পিঁপড়া" },
  { ch: "🕊️", en: "dove peace", bn: "শান্তি পায়রা" },
  { ch: "🍚", en: "rice cooked bowl", bn: "ভাত" },
  { ch: "🍛", en: "curry rice", bn: "ভাত তরকারি" },
  { ch: "🥭", en: "mango fruit", bn: "আম" },
  { ch: "🍌", en: "banana fruit", bn: "কলা" },
  { ch: "🍎", en: "apple fruit red", bn: "আপেল" },
  { ch: "🍇", en: "grapes fruit", bn: "আঙুর" },
  { ch: "🍉", en: "watermelon fruit", bn: "তরমুজ" },
  { ch: "🥥", en: "coconut fruit", bn: "নারকেল" },
  { ch: "🍵", en: "tea green cup", bn: "চা" },
  { ch: "☕", en: "coffee hot cup", bn: "কফি চা" },
  { ch: "🥛", en: "milk glass", bn: "দুধ" },
  { ch: "🍰", en: "cake sweet dessert", bn: "কেক মিষ্টি" },
  { ch: "🍬", en: "candy sweet", bn: "মিষ্টি চকলেট" },
  { ch: "🍞", en: "bread toast", bn: "রুটি পাউরুটি" },
  { ch: "🌸", en: "blossom flower pink", bn: "ফুল" },
  { ch: "🌹", en: "rose flower red love", bn: "গোলাপ ফুল" },
  { ch: "🌻", en: "sunflower flower", bn: "সূর্যমুখী ফুল" },
  { ch: "🌳", en: "tree green", bn: "গাছ" },
  { ch: "🌙", en: "moon crescent night", bn: "চাঁদ রাত" },
  { ch: "☀️", en: "sun sunny day", bn: "সূর্য রোদ" },
  { ch: "🌈", en: "rainbow colors", bn: "রংধনু" },
  { ch: "🌧️", en: "rain cloud weather", bn: "বৃষ্টি" },
  { ch: "⛈️", en: "storm thunder rain", bn: "ঝড় বৃষ্টি" },
  { ch: "❄️", en: "snow winter cold", bn: "বরফ শীত" },
  { ch: "🌊", en: "wave ocean sea", bn: "ঢেউ সাগর" },
  { ch: "🍃", en: "leaves green nature", bn: "পাতা" },
  { ch: "📱", en: "phone mobile smartphone", bn: "মোবাইল ফোন" },
  { ch: "💻", en: "laptop computer", bn: "কম্পিউটার ল্যাপটপ" },
  { ch: "📷", en: "camera photo", bn: "ক্যামেরা ছবি" },
  { ch: "✏️", en: "pencil write edit", bn: "পেন্সিল লেখা" },
  { ch: "📚", en: "books study read", bn: "বই পড়া" },
  { ch: "⚽", en: "football soccer sport", bn: "ফুটবল খেলা" },
  { ch: "🏏", en: "cricket sport bat ball", bn: "ক্রিকেট খেলা" },
  { ch: "🎉", en: "party popper celebrate", bn: "উৎসব পার্টি" },
  { ch: "🎁", en: "gift present surprise", bn: "উপহার" },
  { ch: "🎂", en: "birthday cake celebrate", bn: "জন্মদিন কেক" },
  { ch: "🚗", en: "car drive vehicle", bn: "গাড়ি" },
  { ch: "✈️", en: "plane airplane flight travel", bn: "বিমান ভ্রমণ" },
  { ch: "🚂", en: "train rail", bn: "ট্রেন রেল" },
  { ch: "⛵", en: "boat sail ship", bn: "নৌকা জাহাজ" },
  { ch: "🏠", en: "house home", bn: "বাড়ি বাসা" },
  { ch: "🕌", en: "mosque prayer islam", bn: "মসজিদ নামাজ" },
  { ch: "🛕", en: "temple mandir prayer", bn: "মন্দির পূজা" },
  { ch: "💡", en: "bulb idea light", bn: "বাতি আইডিয়া" },
  { ch: "🔑", en: "key lock", bn: "চাবি তালা" },
  { ch: "🎓", en: "graduation cap degree", bn: "স্নাতক ডিগ্রি" },
  { ch: "🧸", en: "teddy bear toy", bn: "পুতুল খেলনা" },
  { ch: "🪔", en: "diya lamp light", bn: "প্রদীপ বাতি" },
  { ch: "🎈", en: "balloon party", bn: "বেলুন" },
  { ch: "🎆", en: "fireworks celebration", bn: "আতশবাজি উৎসব" },
  { ch: "🏆", en: "trophy winner champion", bn: "ট্রফি পুরস্কার" },
  { ch: "🥇", en: "medal gold winner first", bn: "পদক পুরস্কার" },
  { ch: "💰", en: "money bag cash taka", bn: "টাকা" },
  { ch: "🙌", en: "celebrate raise hands hooray", bn: "উৎসব আনন্দ" },
];

/**
 * Search the bundled emoji list. Every whitespace-separated token must match
 * somewhere in the English keywords, Bengali keywords, or the character
 * itself. Empty queries return the head of the list.
 */
export function searchEmoji(query: string, limit: number = 60): EmojiEntry[] {
  const tokens = query.trim().toLowerCase().split(/\s+/).filter((token) => token.length > 0);
  if (tokens.length === 0) {
    return EMOJI.slice(0, limit);
  }
  return EMOJI.filter((entry) => {
    const haystack = `${entry.en} ${entry.bn} ${entry.ch}`.toLowerCase();
    return tokens.every((token) => haystack.includes(token));
  }).slice(0, limit);
}
