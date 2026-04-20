const form = document.getElementById("search-form");
const queryInput = document.getElementById("query");
const searchButton = document.getElementById("search-button");
const statusEl = document.getElementById("status");
const resultCountEl = document.getElementById("result-count");
const matchedTagsEl = document.getElementById("matched-tags");
const resultsEl = document.getElementById("results");
const template = document.getElementById("result-template");
const availableTagsEl = document.getElementById("available-tags");
const suggestionsEl = document.getElementById("search-suggestions");

const HUMAN_LEVEL_ORDER = [
  "junior",
  "mid level",
  "senior",
  "staff",
  "senior staff",
  "principal",
];

const HUMAN_LEVEL_RANK = new Map(
  HUMAN_LEVEL_ORDER.map((name, index) => [name, index]),
);

const hiddenTagMap = new Map();
const suggestionTagMap = new Map();

let latestSuggestions = [];
let suggestionRequestId = 0;
let suggestionApiAvailable = true;

function setStatus(message, type = "info") {
  statusEl.textContent = message;
  statusEl.classList.toggle("error", type === "error");
}

function clearResults() {
  resultsEl.innerHTML = "";
  resultCountEl.textContent = "";
  matchedTagsEl.textContent = "";
}

function renderEmptyState(message) {
  clearResults();
  const item = document.createElement("li");
  item.className = "empty-state";
  item.textContent = message;
  resultsEl.appendChild(item);
}

function buildProblemUrl(result) {
  if (typeof result.url === "string" && result.url.length > 0) {
    return result.url;
  }

  if (typeof result.slug === "string" && result.slug.length > 0) {
    return `https://leetcode.com/problems/${result.slug}/`;
  }

  return "#";
}

function normalizeTagValue(value) {
  return String(value || "")
    .trim()
    .toLowerCase()
    .replace(/[_-]+/g, " ")
    .replace(/\s+/g, " ");
}

function getTagName(tag) {
  if (typeof tag === "string") {
    return tag.trim();
  }

  if (tag && typeof tag === "object" && typeof tag.name === "string") {
    return tag.name.trim();
  }

  return "";
}

function getTagCategory(tag) {
  if (tag && typeof tag === "object" && typeof tag.category === "string") {
    return tag.category;
  }

  return "";
}

function compareTagNames(left, right) {
  const leftNormalized = normalizeTagValue(left);
  const rightNormalized = normalizeTagValue(right);
  const leftRank = HUMAN_LEVEL_RANK.get(leftNormalized);
  const rightRank = HUMAN_LEVEL_RANK.get(rightNormalized);

  if (leftRank !== undefined || rightRank !== undefined) {
    if (leftRank === undefined) {
      return 1;
    }

    if (rightRank === undefined) {
      return -1;
    }

    return leftRank - rightRank;
  }

  return left.localeCompare(right, undefined, { sensitivity: "base" });
}

function storeTags(targetMap, tags) {
  for (const tag of tags) {
    const name = getTagName(tag);
    if (!name) {
      continue;
    }

    const key = normalizeTagValue(name);
    const existing = targetMap.get(key);

    targetMap.set(key, {
      name,
      category: getTagCategory(tag) || existing?.category || "",
    });
  }
}

function getUniqueSortedTagNames(tags) {
  const seen = new Map();

  for (const tag of tags) {
    const name = getTagName(tag);
    if (!name) {
      continue;
    }

    seen.set(normalizeTagValue(name), name);
  }

  return Array.from(seen.values()).sort(compareTagNames);
}

function formatTag(tag) {
  const name = getTagName(tag);
  const category = getTagCategory(tag);

  if (!name) {
    return "";
  }

  if (!category || category === "topic") {
    return name;
  }

  return `${name} (${category.replaceAll("_", " ")})`;
}

function difficultyClass(difficulty) {
  switch (String(difficulty || "").trim().toLowerCase()) {
    case "easy":
      return "easy";
    case "medium":
      return "medium";
    case "hard":
      return "hard";
    default:
      return "unknown";
  }
}

function buildMetaLine(result) {
  const acceptance =
    typeof result.acceptance === "number"
      ? `${result.acceptance.toFixed(1)}% acceptance`
      : "Acceptance unavailable";

  return [acceptance, result.paid_only ? "Premium" : ""].filter(Boolean);
}

function hideSuggestions() {
  latestSuggestions = [];
  suggestionsEl.hidden = true;
  suggestionsEl.innerHTML = "";
  queryInput.setAttribute("aria-expanded", "false");
}

function applySuggestion(tagName) {
  queryInput.value = tagName;
  hideSuggestions();
  form.requestSubmit();
}

function createSuggestionNode(suggestion) {
  const item = document.createElement("button");
  item.type = "button";
  item.className = "suggestion-item";
  item.setAttribute("role", "option");

  const name = document.createElement("span");
  name.className = "suggestion-name";
  name.textContent = suggestion.name;

  const type = document.createElement("span");
  type.className = "suggestion-type";
  type.textContent = suggestion.category === "topic" ? "topic" : "level";

  item.append(name, type);
  item.addEventListener("mousedown", (event) => {
    event.preventDefault();
    applySuggestion(suggestion.name);
  });

  return item;
}

function renderSuggestions(suggestions) {
  latestSuggestions = suggestions;
  suggestionsEl.innerHTML = "";

  if (suggestions.length === 0) {
    hideSuggestions();
    return;
  }

  const fragment = document.createDocumentFragment();

  for (const suggestion of suggestions) {
    fragment.appendChild(createSuggestionNode(suggestion));
  }

  suggestionsEl.appendChild(fragment);
  suggestionsEl.hidden = false;
  queryInput.setAttribute("aria-expanded", "true");
}

function renderAvailableTags() {
  availableTagsEl.innerHTML = "";

  const sortedTags = Array.from(hiddenTagMap.values()).sort((left, right) =>
    compareTagNames(left.name, right.name),
  );

  for (const tag of sortedTags) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "tag-chip";
    button.textContent = tag.name;
    button.addEventListener("click", () => {
      queryInput.value = tag.name;
      hideSuggestions();
      form.requestSubmit();
    });
    availableTagsEl.appendChild(button);
  }
}

function syncTagsFromPayload(payload) {
  const discoveredTags = [];

  if (Array.isArray(payload.tags)) {
    storeTags(hiddenTagMap, payload.tags);
    discoveredTags.push(...payload.tags);
  }

  if (Array.isArray(payload.available_hidden_tags)) {
    storeTags(hiddenTagMap, payload.available_hidden_tags);
    discoveredTags.push(...payload.available_hidden_tags);
  }

  if (Array.isArray(payload.suggestions)) {
    discoveredTags.push(...payload.suggestions);
  }

  if (Array.isArray(payload.matched_tags)) {
    discoveredTags.push(...payload.matched_tags);
  }

  if (Array.isArray(payload.results)) {
    for (const result of payload.results) {
      if (Array.isArray(result?.matched_tags)) {
        discoveredTags.push(...result.matched_tags);
      }

      if (Array.isArray(result?.tags)) {
        discoveredTags.push(...result.tags);
      }
    }
  }

  storeTags(suggestionTagMap, discoveredTags);
}

function renderResults(payload) {
  const results = Array.isArray(payload.results) ? payload.results : [];
  const matchedTags = Array.isArray(payload.matched_tags) ? payload.matched_tags : [];

  syncTagsFromPayload(payload);
  renderAvailableTags();
  clearResults();

  if (matchedTags.length > 0) {
    matchedTagsEl.textContent = `Matched tags: ${matchedTags
      .map(formatTag)
      .filter(Boolean)
      .sort(compareTagNames)
      .join(", ")}`;
  }

  if (results.length === 0) {
    renderEmptyState("No matching questions found.");
    return;
  }

  const fragment = document.createDocumentFragment();

  for (const result of results) {
    const node = template.content.firstElementChild.cloneNode(true);
    const link = node.querySelector(".result-link");
    const id = node.querySelector(".result-id");
    const meta = node.querySelector(".result-meta");
    const matched = node.querySelector(".result-matched");
    const tags = node.querySelector(".result-tags");
    const badge = node.querySelector(".difficulty-badge");

    const title = result.title || "Untitled problem";
    const matchedNames = getUniqueSortedTagNames(
      Array.isArray(result.matched_tags) ? result.matched_tags : [],
    );
    const matchedNameSet = new Set(matchedNames.map(normalizeTagValue));
    const tagNames = getUniqueSortedTagNames(
      Array.isArray(result.tags) ? result.tags : [],
    ).filter((tag) => !matchedNameSet.has(normalizeTagValue(tag)));

    link.textContent = title;
    link.href = buildProblemUrl(result);
    id.textContent = result.id ? `#${result.id}` : "";

    badge.textContent = result.difficulty || "Unknown";
    badge.dataset.difficulty = difficultyClass(result.difficulty);

    meta.innerHTML = "";
    for (const detail of buildMetaLine(result)) {
      const pill = document.createElement("span");
      pill.className = "meta-pill";
      pill.textContent = detail;
      meta.appendChild(pill);
    }

    matched.textContent = matchedNames.length > 0 ? `Matched: ${matchedNames.join(", ")}` : "";
    matched.hidden = matchedNames.length === 0;

    tags.textContent = tagNames.length > 0 ? `Tags: ${tagNames.join(", ")}` : "";
    tags.hidden = tagNames.length === 0;

    fragment.appendChild(node);
  }

  resultsEl.appendChild(fragment);
  resultCountEl.textContent = `${results.length} result${results.length === 1 ? "" : "s"}`;
}

async function fetchJson(url) {
  const response = await fetch(url, {
    headers: {
      Accept: "application/json",
    },
  });

  const payload = await response.json().catch(() => ({}));

  if (!response.ok) {
    throw new Error(payload.error || `Request failed with status ${response.status}`);
  }

  return payload;
}

async function performSearch(query) {
  const params = new URLSearchParams({ q: query });
  return fetchJson(`/api/search?${params.toString()}`);
}

async function loadAvailableTags() {
  try {
    const payload = await fetchJson("/api/tags");
    syncTagsFromPayload(payload);
    renderAvailableTags();
  } catch (error) {
    availableTagsEl.textContent = error.message || "Unable to load hidden tags.";
  }
}

function getLocalSuggestions(query) {
  const normalizedQuery = normalizeTagValue(query);

  if (!normalizedQuery) {
    return [];
  }

  return Array.from(suggestionTagMap.values())
    .map((suggestion) => {
      const normalizedName = normalizeTagValue(suggestion.name);
      return {
        ...suggestion,
        score: normalizedName.startsWith(normalizedQuery)
          ? 0
          : normalizedName.includes(normalizedQuery)
            ? 1
            : 2,
      };
    })
    .filter((suggestion) => suggestion.score < 2)
    .sort((left, right) => {
      if (left.score !== right.score) {
        return left.score - right.score;
      }

      return compareTagNames(left.name, right.name);
    })
    .slice(0, 8);
}

function mergeSuggestions(primary, fallback) {
  const merged = new Map();

  for (const suggestion of [...primary, ...fallback]) {
    const name = getTagName(suggestion);
    if (!name) {
      continue;
    }

    const key = normalizeTagValue(name);
    if (!merged.has(key)) {
      merged.set(key, {
        name,
        category: getTagCategory(suggestion),
      });
    }
  }

  return Array.from(merged.values()).slice(0, 8);
}

async function loadSuggestions(query) {
  const trimmedQuery = query.trim();
  if (trimmedQuery.length === 0) {
    hideSuggestions();
    return;
  }

  const requestId = ++suggestionRequestId;
  const fallbackSuggestions = getLocalSuggestions(trimmedQuery);

  if (suggestionApiAvailable) {
    try {
      const params = new URLSearchParams({ q: trimmedQuery });
      const payload = await fetchJson(`/api/suggest?${params.toString()}`);

      if (requestId !== suggestionRequestId) {
        return;
      }

      syncTagsFromPayload(payload);
      const remoteSuggestions = Array.isArray(payload.suggestions) ? payload.suggestions : [];
      renderSuggestions(mergeSuggestions(remoteSuggestions, fallbackSuggestions));
      return;
    } catch (_error) {
      suggestionApiAvailable = false;
    }
  }

  if (requestId === suggestionRequestId) {
    renderSuggestions(fallbackSuggestions);
  }
}

queryInput.addEventListener("input", () => {
  loadSuggestions(queryInput.value);
});

queryInput.addEventListener("focus", () => {
  if (queryInput.value.trim()) {
    loadSuggestions(queryInput.value);
  }
});

queryInput.addEventListener("keydown", (event) => {
  if (event.key === "Escape") {
    hideSuggestions();
    return;
  }

  if (event.key === "ArrowDown" && latestSuggestions.length > 0) {
    event.preventDefault();
    suggestionsEl.querySelector(".suggestion-item")?.focus();
  }
});

suggestionsEl.addEventListener("keydown", (event) => {
  const items = Array.from(suggestionsEl.querySelectorAll(".suggestion-item"));
  const index = items.indexOf(document.activeElement);

  if (event.key === "ArrowDown") {
    event.preventDefault();
    items[Math.min(index + 1, items.length - 1)]?.focus();
  } else if (event.key === "ArrowUp") {
    event.preventDefault();
    if (index <= 0) {
      queryInput.focus();
    } else {
      items[index - 1]?.focus();
    }
  } else if (event.key === "Escape") {
    hideSuggestions();
    queryInput.focus();
  }
});

document.addEventListener("click", (event) => {
  if (!event.target.closest(".search-input-wrap")) {
    hideSuggestions();
  }
});

form.addEventListener("submit", async (event) => {
  event.preventDefault();

  const query = queryInput.value.trim();
  if (!query) {
    setStatus("Enter a tag to search.", "error");
    renderEmptyState("Start by entering a tag.");
    return;
  }

  hideSuggestions();
  searchButton.disabled = true;
  setStatus(`Searching for "${query}"...`);
  renderEmptyState("Building results...");

  try {
    const payload = await performSearch(query);
    renderResults(payload);
    setStatus(`Finished searching for "${query}".`);
  } catch (error) {
    renderEmptyState("The backend did not return results.");
    setStatus(error.message || "Search failed.", "error");
  } finally {
    searchButton.disabled = false;
  }
});

renderEmptyState("Start by entering a tag.");
loadAvailableTags();
