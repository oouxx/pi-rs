// @bun
// node_modules/.bun/get-east-asian-width@1.6.0/node_modules/get-east-asian-width/lookup-data.js
var ambiguousMinimalCodePoint = 161;
var ambiguousMaximumCodePoint = 1114109;
var ambiguousRanges = [161, 161, 164, 164, 167, 168, 170, 170, 173, 174, 176, 180, 182, 186, 188, 191, 198, 198, 208, 208, 215, 216, 222, 225, 230, 230, 232, 234, 236, 237, 240, 240, 242, 243, 247, 250, 252, 252, 254, 254, 257, 257, 273, 273, 275, 275, 283, 283, 294, 295, 299, 299, 305, 307, 312, 312, 319, 322, 324, 324, 328, 331, 333, 333, 338, 339, 358, 359, 363, 363, 462, 462, 464, 464, 466, 466, 468, 468, 470, 470, 472, 472, 474, 474, 476, 476, 593, 593, 609, 609, 708, 708, 711, 711, 713, 715, 717, 717, 720, 720, 728, 731, 733, 733, 735, 735, 768, 879, 913, 929, 931, 937, 945, 961, 963, 969, 1025, 1025, 1040, 1103, 1105, 1105, 8208, 8208, 8211, 8214, 8216, 8217, 8220, 8221, 8224, 8226, 8228, 8231, 8240, 8240, 8242, 8243, 8245, 8245, 8251, 8251, 8254, 8254, 8308, 8308, 8319, 8319, 8321, 8324, 8364, 8364, 8451, 8451, 8453, 8453, 8457, 8457, 8467, 8467, 8470, 8470, 8481, 8482, 8486, 8486, 8491, 8491, 8531, 8532, 8539, 8542, 8544, 8555, 8560, 8569, 8585, 8585, 8592, 8601, 8632, 8633, 8658, 8658, 8660, 8660, 8679, 8679, 8704, 8704, 8706, 8707, 8711, 8712, 8715, 8715, 8719, 8719, 8721, 8721, 8725, 8725, 8730, 8730, 8733, 8736, 8739, 8739, 8741, 8741, 8743, 8748, 8750, 8750, 8756, 8759, 8764, 8765, 8776, 8776, 8780, 8780, 8786, 8786, 8800, 8801, 8804, 8807, 8810, 8811, 8814, 8815, 8834, 8835, 8838, 8839, 8853, 8853, 8857, 8857, 8869, 8869, 8895, 8895, 8978, 8978, 9312, 9449, 9451, 9547, 9552, 9587, 9600, 9615, 9618, 9621, 9632, 9633, 9635, 9641, 9650, 9651, 9654, 9655, 9660, 9661, 9664, 9665, 9670, 9672, 9675, 9675, 9678, 9681, 9698, 9701, 9711, 9711, 9733, 9734, 9737, 9737, 9742, 9743, 9756, 9756, 9758, 9758, 9792, 9792, 9794, 9794, 9824, 9825, 9827, 9829, 9831, 9834, 9836, 9837, 9839, 9839, 9886, 9887, 9919, 9919, 9926, 9933, 9935, 9939, 9941, 9953, 9955, 9955, 9960, 9961, 9963, 9969, 9972, 9972, 9974, 9977, 9979, 9980, 9982, 9983, 10045, 10045, 10102, 10111, 11094, 11097, 12872, 12879, 57344, 63743, 65024, 65039, 65533, 65533, 127232, 127242, 127248, 127277, 127280, 127337, 127344, 127373, 127375, 127376, 127387, 127404, 917760, 917999, 983040, 1048573, 1048576, 1114109];
var fullwidthMinimalCodePoint = 12288;
var fullwidthMaximumCodePoint = 65510;
var fullwidthRanges = [12288, 12288, 65281, 65376, 65504, 65510];
var wideMinimalCodePoint = 4352;
var wideMaximumCodePoint = 262141;
var wideRanges = [4352, 4447, 8986, 8987, 9001, 9002, 9193, 9196, 9200, 9200, 9203, 9203, 9725, 9726, 9748, 9749, 9776, 9783, 9800, 9811, 9855, 9855, 9866, 9871, 9875, 9875, 9889, 9889, 9898, 9899, 9917, 9918, 9924, 9925, 9934, 9934, 9940, 9940, 9962, 9962, 9970, 9971, 9973, 9973, 9978, 9978, 9981, 9981, 9989, 9989, 9994, 9995, 10024, 10024, 10060, 10060, 10062, 10062, 10067, 10069, 10071, 10071, 10133, 10135, 10160, 10160, 10175, 10175, 11035, 11036, 11088, 11088, 11093, 11093, 11904, 11929, 11931, 12019, 12032, 12245, 12272, 12287, 12289, 12350, 12353, 12438, 12441, 12543, 12549, 12591, 12593, 12686, 12688, 12773, 12783, 12830, 12832, 12871, 12880, 42124, 42128, 42182, 43360, 43388, 44032, 55203, 63744, 64255, 65040, 65049, 65072, 65106, 65108, 65126, 65128, 65131, 94176, 94180, 94192, 94198, 94208, 101589, 101631, 101662, 101760, 101874, 110576, 110579, 110581, 110587, 110589, 110590, 110592, 110882, 110898, 110898, 110928, 110930, 110933, 110933, 110948, 110951, 110960, 111355, 119552, 119638, 119648, 119670, 126980, 126980, 127183, 127183, 127374, 127374, 127377, 127386, 127488, 127490, 127504, 127547, 127552, 127560, 127568, 127569, 127584, 127589, 127744, 127776, 127789, 127797, 127799, 127868, 127870, 127891, 127904, 127946, 127951, 127955, 127968, 127984, 127988, 127988, 127992, 128062, 128064, 128064, 128066, 128252, 128255, 128317, 128331, 128334, 128336, 128359, 128378, 128378, 128405, 128406, 128420, 128420, 128507, 128591, 128640, 128709, 128716, 128716, 128720, 128722, 128725, 128728, 128732, 128735, 128747, 128748, 128756, 128764, 128992, 129003, 129008, 129008, 129292, 129338, 129340, 129349, 129351, 129535, 129648, 129660, 129664, 129674, 129678, 129734, 129736, 129736, 129741, 129756, 129759, 129770, 129775, 129784, 131072, 196605, 196608, 262141];

// node_modules/.bun/get-east-asian-width@1.6.0/node_modules/get-east-asian-width/utilities.js
var isInRange = (ranges, codePoint) => {
  let low = 0;
  let high = Math.floor(ranges.length / 2) - 1;
  while (low <= high) {
    const mid = Math.floor((low + high) / 2);
    const i = mid * 2;
    if (codePoint < ranges[i]) {
      high = mid - 1;
    } else if (codePoint > ranges[i + 1]) {
      low = mid + 1;
    } else {
      return true;
    }
  }
  return false;
};

// node_modules/.bun/get-east-asian-width@1.6.0/node_modules/get-east-asian-width/lookup.js
var commonCjkCodePoint = 19968;
var [wideFastPathStart, wideFastPathEnd] = /* @__PURE__ */ findWideFastPathRange(wideRanges);
function findWideFastPathRange(ranges) {
  let fastPathStart = ranges[0];
  let fastPathEnd = ranges[1];
  for (let index = 0;index < ranges.length; index += 2) {
    const start = ranges[index];
    const end = ranges[index + 1];
    if (commonCjkCodePoint >= start && commonCjkCodePoint <= end) {
      return [start, end];
    }
    if (end - start > fastPathEnd - fastPathStart) {
      fastPathStart = start;
      fastPathEnd = end;
    }
  }
  return [fastPathStart, fastPathEnd];
}
var isAmbiguous = (codePoint) => {
  if (codePoint < ambiguousMinimalCodePoint || codePoint > ambiguousMaximumCodePoint) {
    return false;
  }
  return isInRange(ambiguousRanges, codePoint);
};
var isFullWidth = (codePoint) => {
  if (codePoint < fullwidthMinimalCodePoint || codePoint > fullwidthMaximumCodePoint) {
    return false;
  }
  return isInRange(fullwidthRanges, codePoint);
};
var isWide = (codePoint) => {
  if (codePoint >= wideFastPathStart && codePoint <= wideFastPathEnd) {
    return true;
  }
  if (codePoint < wideMinimalCodePoint || codePoint > wideMaximumCodePoint) {
    return false;
  }
  return isInRange(wideRanges, codePoint);
};

// node_modules/.bun/get-east-asian-width@1.6.0/node_modules/get-east-asian-width/index.js
function validate(codePoint) {
  if (!Number.isSafeInteger(codePoint)) {
    throw new TypeError(`Expected a code point, got \`${typeof codePoint}\`.`);
  }
}
function eastAsianWidth(codePoint, { ambiguousAsWide = false } = {}) {
  validate(codePoint);
  if (isFullWidth(codePoint) || isWide(codePoint) || ambiguousAsWide && isAmbiguous(codePoint)) {
    return 2;
  }
  return 1;
}

// packages/tui/src/utils.ts
var graphemeSegmenter = new Intl.Segmenter(undefined, { granularity: "grapheme" });
var wordSegmenter = new Intl.Segmenter(undefined, { granularity: "word" });
function couldBeEmoji(segment) {
  const cp = segment.codePointAt(0);
  return cp >= 126976 && cp <= 130047 || cp >= 8960 && cp <= 9215 || cp >= 9728 && cp <= 10175 || cp >= 11088 && cp <= 11093 || segment.includes("\uFE0F") || segment.length > 2;
}
var zeroWidthRegex = /^(?:\p{Default_Ignorable_Code_Point}|\p{Control}|\p{Mark}|\p{Surrogate})+$/v;
var leadingNonPrintingRegex = /^[\p{Default_Ignorable_Code_Point}\p{Control}\p{Format}\p{Mark}\p{Surrogate}]+/v;
var nonPrintingCharRegex = /^(?:\p{Default_Ignorable_Code_Point}|\p{Control}|\p{Format}|\p{Mark}|\p{Surrogate})$/v;
var markCharRegex = /^\p{Mark}$/v;
var terminalSpacingMarkRegex = /^(?:[\p{Spacing_Mark}--[\u1734\u302E\u302F]]|[\u065F\u0F7F\u102B\u102C\u1031\u1033-\u1035\u1038\u103A-\u103E])+$/v;
var rgiEmojiRegex = /^\p{RGI_Emoji}$/v;
var WIDTH_CACHE_SIZE = 512;
var widthCache = new Map;
function isPrintableAscii(str) {
  for (let i = 0;i < str.length; i++) {
    const code = str.charCodeAt(i);
    if (code < 32 || code > 126) {
      return false;
    }
  }
  return true;
}
function truncateFragmentToWidth(text, maxWidth) {
  if (maxWidth <= 0 || text.length === 0) {
    return { text: "", width: 0 };
  }
  if (isPrintableAscii(text)) {
    const clipped = text.slice(0, maxWidth);
    return { text: clipped, width: clipped.length };
  }
  const hasAnsi = text.includes("\x1B");
  const hasTabs = text.includes("\t");
  if (!hasAnsi && !hasTabs) {
    let result2 = "";
    let width2 = 0;
    for (const { segment } of graphemeSegmenter.segment(text)) {
      const w = graphemeWidth(segment);
      if (width2 + w > maxWidth) {
        break;
      }
      result2 += segment;
      width2 += w;
    }
    return { text: result2, width: width2 };
  }
  let result = "";
  let width = 0;
  let i = 0;
  let pendingAnsi = "";
  while (i < text.length) {
    const ansi = extractAnsiCode(text, i);
    if (ansi) {
      pendingAnsi += ansi.code;
      i += ansi.length;
      continue;
    }
    if (text[i] === "\t") {
      if (width + 3 > maxWidth) {
        break;
      }
      if (pendingAnsi) {
        result += pendingAnsi;
        pendingAnsi = "";
      }
      result += "\t";
      width += 3;
      i++;
      continue;
    }
    let end = i;
    while (end < text.length && text[end] !== "\t") {
      const nextAnsi = extractAnsiCode(text, end);
      if (nextAnsi) {
        break;
      }
      end++;
    }
    for (const { segment } of graphemeSegmenter.segment(text.slice(i, end))) {
      const w = graphemeWidth(segment);
      if (width + w > maxWidth) {
        return { text: result, width };
      }
      if (pendingAnsi) {
        result += pendingAnsi;
        pendingAnsi = "";
      }
      result += segment;
      width += w;
    }
    i = end;
  }
  return { text: result, width };
}
function finalizeTruncatedResult(prefix, prefixWidth, ellipsis, ellipsisWidth, maxWidth, pad) {
  const reset = "\x1B[0m";
  const hyperlinkClose = getActiveOsc8Close(prefix);
  const visibleWidth = prefixWidth + ellipsisWidth;
  let result;
  if (ellipsis.length > 0) {
    result = `${prefix}${hyperlinkClose}${reset}${ellipsis}${reset}`;
  } else {
    result = `${prefix}${hyperlinkClose}${reset}`;
  }
  return pad ? result + " ".repeat(Math.max(0, maxWidth - visibleWidth)) : result;
}
function graphemeWidth(segment) {
  if (segment === "\t") {
    return 3;
  }
  if (terminalSpacingMarkRegex.test(segment)) {
    return [...segment].length;
  }
  if (zeroWidthRegex.test(segment)) {
    return 0;
  }
  if (couldBeEmoji(segment) && rgiEmojiRegex.test(segment)) {
    return 2;
  }
  const base = segment.replace(leadingNonPrintingRegex, "");
  const cp = base.codePointAt(0);
  if (cp === undefined) {
    return 0;
  }
  if (cp >= 127462 && cp <= 127487) {
    return 2;
  }
  let width = eastAsianWidth(cp);
  let followsMark = false;
  const chars = [...base];
  for (const char of chars.slice(1)) {
    if (terminalSpacingMarkRegex.test(char)) {
      width += 1;
      followsMark = false;
    } else if (markCharRegex.test(char)) {
      followsMark = true;
    } else if (!nonPrintingCharRegex.test(char)) {
      const c = char.codePointAt(0);
      if (followsMark || c >= 65280 && c <= 65519) {
        width += eastAsianWidth(c);
      } else if (c === 3635 || c === 3763) {
        width += 1;
      }
      followsMark = false;
    }
  }
  return width;
}
function visibleWidth(str) {
  if (str.length === 0) {
    return 0;
  }
  if (isPrintableAscii(str)) {
    return str.length;
  }
  const cached = widthCache.get(str);
  if (cached !== undefined) {
    return cached;
  }
  let clean = str;
  if (str.includes("\t")) {
    clean = clean.replace(/\t/g, "   ");
  }
  if (clean.includes("\x1B")) {
    let stripped = "";
    let i = 0;
    while (i < clean.length) {
      const ansi = extractAnsiCode(clean, i);
      if (ansi) {
        i += ansi.length;
        continue;
      }
      stripped += clean[i];
      i++;
    }
    clean = stripped;
  }
  let width = 0;
  for (const { segment } of graphemeSegmenter.segment(clean)) {
    width += graphemeWidth(segment);
  }
  if (widthCache.size >= WIDTH_CACHE_SIZE) {
    const firstKey = widthCache.keys().next().value;
    if (firstKey !== undefined) {
      widthCache.delete(firstKey);
    }
  }
  widthCache.set(str, width);
  return width;
}
function stripTerminalSequences(str) {
  if (!str.includes("\x1B"))
    return str;
  let result = "";
  let i = 0;
  while (i < str.length) {
    const ansi = extractAnsiCode(str, i);
    if (ansi) {
      i += ansi.length;
      continue;
    }
    result += str[i];
    i++;
  }
  return result;
}
function extractAnsiCode(str, pos) {
  if (pos >= str.length || str[pos] !== "\x1B")
    return null;
  const next = str[pos + 1];
  if (next === "[") {
    let j = pos + 2;
    while (j < str.length && !/[mGKHJ]/.test(str[j]))
      j++;
    if (j < str.length)
      return { code: str.substring(pos, j + 1), length: j + 1 - pos };
    return null;
  }
  if (next === "]") {
    let j = pos + 2;
    while (j < str.length) {
      if (str[j] === "\x07")
        return { code: str.substring(pos, j + 1), length: j + 1 - pos };
      if (str[j] === "\x1B" && str[j + 1] === "\\")
        return { code: str.substring(pos, j + 2), length: j + 2 - pos };
      j++;
    }
    return null;
  }
  if (next === "_") {
    let j = pos + 2;
    while (j < str.length) {
      if (str[j] === "\x07")
        return { code: str.substring(pos, j + 1), length: j + 1 - pos };
      if (str[j] === "\x1B" && str[j + 1] === "\\")
        return { code: str.substring(pos, j + 2), length: j + 2 - pos };
      j++;
    }
    return null;
  }
  return null;
}
function parseOsc8Hyperlink(ansiCode) {
  if (!ansiCode.startsWith("\x1B]8;")) {
    return;
  }
  const terminator = ansiCode.endsWith("\x07") ? "\x07" : "\x1B\\";
  const body = ansiCode.slice(4, terminator === "\x07" ? -1 : -2);
  const separatorIndex = body.indexOf(";");
  if (separatorIndex === -1) {
    return;
  }
  const params = body.slice(0, separatorIndex);
  const url = body.slice(separatorIndex + 1);
  if (!url) {
    return null;
  }
  return { params, url, terminator };
}
function formatOsc8Hyperlink(hyperlink) {
  return `\x1B]8;${hyperlink.params};${hyperlink.url}${hyperlink.terminator}`;
}
function formatOsc8Close(terminator) {
  return `\x1B]8;;${terminator}`;
}
function getActiveOsc8Close(prefix) {
  if (!prefix.includes("\x1B]8;")) {
    return "";
  }
  let activeHyperlink = null;
  let i = 0;
  while (i < prefix.length) {
    const ansi = extractAnsiCode(prefix, i);
    if (ansi) {
      const hyperlink = parseOsc8Hyperlink(ansi.code);
      if (hyperlink !== undefined) {
        activeHyperlink = hyperlink;
      }
      i += ansi.length;
    } else {
      i++;
    }
  }
  return activeHyperlink ? formatOsc8Close(activeHyperlink.terminator) : "";
}

class AnsiCodeTracker {
  bold = false;
  dim = false;
  italic = false;
  underline = false;
  blink = false;
  inverse = false;
  hidden = false;
  strikethrough = false;
  fgColor = null;
  bgColor = null;
  activeHyperlink = null;
  process(ansiCode) {
    const hyperlink = parseOsc8Hyperlink(ansiCode);
    if (hyperlink !== undefined) {
      this.activeHyperlink = hyperlink;
      return;
    }
    if (!ansiCode.endsWith("m")) {
      return;
    }
    const match = ansiCode.match(/\x1b\[([\d;]*)m/);
    if (!match)
      return;
    const params = match[1];
    if (params === "" || params === "0") {
      this.reset();
      return;
    }
    const parts = params.split(";");
    let i = 0;
    while (i < parts.length) {
      const code = Number.parseInt(parts[i], 10);
      if (code === 38 || code === 48) {
        if (parts[i + 1] === "5" && parts[i + 2] !== undefined) {
          const colorCode = `${parts[i]};${parts[i + 1]};${parts[i + 2]}`;
          if (code === 38) {
            this.fgColor = colorCode;
          } else {
            this.bgColor = colorCode;
          }
          i += 3;
          continue;
        } else if (parts[i + 1] === "2" && parts[i + 4] !== undefined) {
          const colorCode = `${parts[i]};${parts[i + 1]};${parts[i + 2]};${parts[i + 3]};${parts[i + 4]}`;
          if (code === 38) {
            this.fgColor = colorCode;
          } else {
            this.bgColor = colorCode;
          }
          i += 5;
          continue;
        }
      }
      switch (code) {
        case 0:
          this.reset();
          break;
        case 1:
          this.bold = true;
          break;
        case 2:
          this.dim = true;
          break;
        case 3:
          this.italic = true;
          break;
        case 4:
          this.underline = true;
          break;
        case 5:
          this.blink = true;
          break;
        case 7:
          this.inverse = true;
          break;
        case 8:
          this.hidden = true;
          break;
        case 9:
          this.strikethrough = true;
          break;
        case 21:
          this.bold = false;
          break;
        case 22:
          this.bold = false;
          this.dim = false;
          break;
        case 23:
          this.italic = false;
          break;
        case 24:
          this.underline = false;
          break;
        case 25:
          this.blink = false;
          break;
        case 27:
          this.inverse = false;
          break;
        case 28:
          this.hidden = false;
          break;
        case 29:
          this.strikethrough = false;
          break;
        case 39:
          this.fgColor = null;
          break;
        case 49:
          this.bgColor = null;
          break;
        default:
          if (code >= 30 && code <= 37 || code >= 90 && code <= 97) {
            this.fgColor = String(code);
          } else if (code >= 40 && code <= 47 || code >= 100 && code <= 107) {
            this.bgColor = String(code);
          }
          break;
      }
      i++;
    }
  }
  reset() {
    this.bold = false;
    this.dim = false;
    this.italic = false;
    this.underline = false;
    this.blink = false;
    this.inverse = false;
    this.hidden = false;
    this.strikethrough = false;
    this.fgColor = null;
    this.bgColor = null;
  }
  clear() {
    this.reset();
    this.activeHyperlink = null;
  }
  getActiveCodes() {
    const codes = [];
    if (this.bold)
      codes.push("1");
    if (this.dim)
      codes.push("2");
    if (this.italic)
      codes.push("3");
    if (this.underline)
      codes.push("4");
    if (this.blink)
      codes.push("5");
    if (this.inverse)
      codes.push("7");
    if (this.hidden)
      codes.push("8");
    if (this.strikethrough)
      codes.push("9");
    if (this.fgColor)
      codes.push(this.fgColor);
    if (this.bgColor)
      codes.push(this.bgColor);
    let result = codes.length > 0 ? `\x1B[${codes.join(";")}m` : "";
    if (this.activeHyperlink) {
      result += formatOsc8Hyperlink(this.activeHyperlink);
    }
    return result;
  }
  hasActiveCodes() {
    return this.bold || this.dim || this.italic || this.underline || this.blink || this.inverse || this.hidden || this.strikethrough || this.fgColor !== null || this.bgColor !== null || this.activeHyperlink !== null;
  }
  getLineEndReset() {
    let result = "";
    if (this.underline) {
      result += "\x1B[24m";
    }
    if (this.activeHyperlink) {
      result += formatOsc8Close(this.activeHyperlink.terminator);
    }
    return result;
  }
}
function truncateToWidth(text, maxWidth, ellipsis = "...", pad = false) {
  if (maxWidth <= 0) {
    return "";
  }
  if (text.length === 0) {
    return pad ? " ".repeat(maxWidth) : "";
  }
  const ellipsisWidth = visibleWidth(ellipsis);
  if (ellipsisWidth >= maxWidth) {
    const textWidth = visibleWidth(text);
    if (textWidth <= maxWidth) {
      return pad ? text + " ".repeat(maxWidth - textWidth) : text;
    }
    const clippedEllipsis = truncateFragmentToWidth(ellipsis, maxWidth);
    if (clippedEllipsis.width === 0) {
      return pad ? " ".repeat(maxWidth) : "";
    }
    return finalizeTruncatedResult("", 0, clippedEllipsis.text, clippedEllipsis.width, maxWidth, pad);
  }
  if (isPrintableAscii(text)) {
    if (text.length <= maxWidth) {
      return pad ? text + " ".repeat(maxWidth - text.length) : text;
    }
    const targetWidth2 = maxWidth - ellipsisWidth;
    return finalizeTruncatedResult(text.slice(0, targetWidth2), targetWidth2, ellipsis, ellipsisWidth, maxWidth, pad);
  }
  const targetWidth = maxWidth - ellipsisWidth;
  let result = "";
  let pendingAnsi = "";
  let visibleSoFar = 0;
  let keptWidth = 0;
  let keepContiguousPrefix = true;
  let overflowed = false;
  let exhaustedInput = false;
  const hasAnsi = text.includes("\x1B");
  const hasTabs = text.includes("\t");
  if (!hasAnsi && !hasTabs) {
    for (const { segment } of graphemeSegmenter.segment(text)) {
      const width = graphemeWidth(segment);
      if (keepContiguousPrefix && keptWidth + width <= targetWidth) {
        result += segment;
        keptWidth += width;
      } else {
        keepContiguousPrefix = false;
      }
      visibleSoFar += width;
      if (visibleSoFar > maxWidth) {
        overflowed = true;
        break;
      }
    }
    exhaustedInput = !overflowed;
  } else {
    let i = 0;
    while (i < text.length) {
      const ansi = extractAnsiCode(text, i);
      if (ansi) {
        pendingAnsi += ansi.code;
        i += ansi.length;
        continue;
      }
      if (text[i] === "\t") {
        if (keepContiguousPrefix && keptWidth + 3 <= targetWidth) {
          if (pendingAnsi) {
            result += pendingAnsi;
            pendingAnsi = "";
          }
          result += "\t";
          keptWidth += 3;
        } else {
          keepContiguousPrefix = false;
          pendingAnsi = "";
        }
        visibleSoFar += 3;
        if (visibleSoFar > maxWidth) {
          overflowed = true;
          break;
        }
        i++;
        continue;
      }
      let end = i;
      while (end < text.length && text[end] !== "\t") {
        const nextAnsi = extractAnsiCode(text, end);
        if (nextAnsi) {
          break;
        }
        end++;
      }
      for (const { segment } of graphemeSegmenter.segment(text.slice(i, end))) {
        const width = graphemeWidth(segment);
        if (keepContiguousPrefix && keptWidth + width <= targetWidth) {
          if (pendingAnsi) {
            result += pendingAnsi;
            pendingAnsi = "";
          }
          result += segment;
          keptWidth += width;
        } else {
          keepContiguousPrefix = false;
          pendingAnsi = "";
        }
        visibleSoFar += width;
        if (visibleSoFar > maxWidth) {
          overflowed = true;
          break;
        }
      }
      if (overflowed) {
        break;
      }
      i = end;
    }
    exhaustedInput = i >= text.length;
  }
  if (!overflowed && exhaustedInput) {
    return pad ? text + " ".repeat(Math.max(0, maxWidth - visibleSoFar)) : text;
  }
  return finalizeTruncatedResult(result, keptWidth, ellipsis, ellipsisWidth, maxWidth, pad);
}
var pooledStyleTracker = new AnsiCodeTracker;

// packages/tui/src/keys.ts
var _kittyProtocolActive = false;
var SYMBOL_KEYS = new Set([
  "`",
  "-",
  "=",
  "[",
  "]",
  "\\",
  ";",
  "'",
  ",",
  ".",
  "/",
  "!",
  "@",
  "#",
  "$",
  "%",
  "^",
  "&",
  "*",
  "(",
  ")",
  "_",
  "+",
  "|",
  "~",
  "{",
  "}",
  ":",
  "<",
  ">",
  "?"
]);
var MODIFIERS = {
  shift: 1,
  alt: 2,
  ctrl: 4,
  super: 8
};
var LOCK_MASK = 64 + 128;
var CODEPOINTS = {
  escape: 27,
  tab: 9,
  enter: 13,
  space: 32,
  backspace: 127,
  kpEnter: 57414
};
var ARROW_CODEPOINTS = {
  up: -1,
  down: -2,
  right: -3,
  left: -4
};
var FUNCTIONAL_CODEPOINTS = {
  delete: -10,
  insert: -11,
  pageUp: -12,
  pageDown: -13,
  home: -14,
  end: -15
};
var KITTY_FUNCTIONAL_KEY_EQUIVALENTS = new Map([
  [57399, 48],
  [57400, 49],
  [57401, 50],
  [57402, 51],
  [57403, 52],
  [57404, 53],
  [57405, 54],
  [57406, 55],
  [57407, 56],
  [57408, 57],
  [57409, 46],
  [57410, 47],
  [57411, 42],
  [57412, 45],
  [57413, 43],
  [57415, 61],
  [57416, 44],
  [57417, ARROW_CODEPOINTS.left],
  [57418, ARROW_CODEPOINTS.right],
  [57419, ARROW_CODEPOINTS.up],
  [57420, ARROW_CODEPOINTS.down],
  [57421, FUNCTIONAL_CODEPOINTS.pageUp],
  [57422, FUNCTIONAL_CODEPOINTS.pageDown],
  [57423, FUNCTIONAL_CODEPOINTS.home],
  [57424, FUNCTIONAL_CODEPOINTS.end],
  [57425, FUNCTIONAL_CODEPOINTS.insert],
  [57426, FUNCTIONAL_CODEPOINTS.delete]
]);
function normalizeKittyFunctionalCodepoint(codepoint) {
  return KITTY_FUNCTIONAL_KEY_EQUIVALENTS.get(codepoint) ?? codepoint;
}
function normalizeShiftedLetterIdentityCodepoint(codepoint, modifier) {
  const effectiveModifier = modifier & ~LOCK_MASK;
  if ((effectiveModifier & MODIFIERS.shift) !== 0 && codepoint >= 65 && codepoint <= 90) {
    return codepoint + 32;
  }
  return codepoint;
}
var LEGACY_KEY_SEQUENCES = {
  up: ["\x1B[A", "\x1BOA"],
  down: ["\x1B[B", "\x1BOB"],
  right: ["\x1B[C", "\x1BOC"],
  left: ["\x1B[D", "\x1BOD"],
  home: ["\x1B[H", "\x1BOH", "\x1B[1~", "\x1B[7~"],
  end: ["\x1B[F", "\x1BOF", "\x1B[4~", "\x1B[8~"],
  insert: ["\x1B[2~"],
  delete: ["\x1B[3~"],
  pageUp: ["\x1B[5~", "\x1B[[5~"],
  pageDown: ["\x1B[6~", "\x1B[[6~"],
  clear: ["\x1B[E", "\x1BOE"],
  f1: ["\x1BOP", "\x1B[11~", "\x1B[[A"],
  f2: ["\x1BOQ", "\x1B[12~", "\x1B[[B"],
  f3: ["\x1BOR", "\x1B[13~", "\x1B[[C"],
  f4: ["\x1BOS", "\x1B[14~", "\x1B[[D"],
  f5: ["\x1B[15~", "\x1B[[E"],
  f6: ["\x1B[17~"],
  f7: ["\x1B[18~"],
  f8: ["\x1B[19~"],
  f9: ["\x1B[20~"],
  f10: ["\x1B[21~"],
  f11: ["\x1B[23~"],
  f12: ["\x1B[24~"]
};
var LEGACY_SHIFT_SEQUENCES = {
  up: ["\x1B[a"],
  down: ["\x1B[b"],
  right: ["\x1B[c"],
  left: ["\x1B[d"],
  clear: ["\x1B[e"],
  insert: ["\x1B[2$"],
  delete: ["\x1B[3$"],
  pageUp: ["\x1B[5$"],
  pageDown: ["\x1B[6$"],
  home: ["\x1B[7$"],
  end: ["\x1B[8$"]
};
var LEGACY_CTRL_SEQUENCES = {
  up: ["\x1BOa"],
  down: ["\x1BOb"],
  right: ["\x1BOc"],
  left: ["\x1BOd"],
  clear: ["\x1BOe"],
  insert: ["\x1B[2^"],
  delete: ["\x1B[3^"],
  pageUp: ["\x1B[5^"],
  pageDown: ["\x1B[6^"],
  home: ["\x1B[7^"],
  end: ["\x1B[8^"]
};
var LEGACY_SEQUENCE_KEY_IDS = {
  "\x1BOA": "up",
  "\x1BOB": "down",
  "\x1BOC": "right",
  "\x1BOD": "left",
  "\x1BOH": "home",
  "\x1BOF": "end",
  "\x1B[E": "clear",
  "\x1BOE": "clear",
  "\x1BOe": "ctrl+clear",
  "\x1B[e": "shift+clear",
  "\x1B[2~": "insert",
  "\x1B[2$": "shift+insert",
  "\x1B[2^": "ctrl+insert",
  "\x1B[3$": "shift+delete",
  "\x1B[3^": "ctrl+delete",
  "\x1B[[5~": "pageUp",
  "\x1B[[6~": "pageDown",
  "\x1B[a": "shift+up",
  "\x1B[b": "shift+down",
  "\x1B[c": "shift+right",
  "\x1B[d": "shift+left",
  "\x1BOa": "ctrl+up",
  "\x1BOb": "ctrl+down",
  "\x1BOc": "ctrl+right",
  "\x1BOd": "ctrl+left",
  "\x1B[5$": "shift+pageUp",
  "\x1B[6$": "shift+pageDown",
  "\x1B[7$": "shift+home",
  "\x1B[8$": "shift+end",
  "\x1B[5^": "ctrl+pageUp",
  "\x1B[6^": "ctrl+pageDown",
  "\x1B[7^": "ctrl+home",
  "\x1B[8^": "ctrl+end",
  "\x1BOP": "f1",
  "\x1BOQ": "f2",
  "\x1BOR": "f3",
  "\x1BOS": "f4",
  "\x1B[11~": "f1",
  "\x1B[12~": "f2",
  "\x1B[13~": "f3",
  "\x1B[14~": "f4",
  "\x1B[[A": "f1",
  "\x1B[[B": "f2",
  "\x1B[[C": "f3",
  "\x1B[[D": "f4",
  "\x1B[[E": "f5",
  "\x1B[15~": "f5",
  "\x1B[17~": "f6",
  "\x1B[18~": "f7",
  "\x1B[19~": "f8",
  "\x1B[20~": "f9",
  "\x1B[21~": "f10",
  "\x1B[23~": "f11",
  "\x1B[24~": "f12",
  "\x1Bb": "alt+left",
  "\x1Bf": "alt+right",
  "\x1Bp": "alt+up",
  "\x1Bn": "alt+down"
};
var matchesLegacySequence = (data, sequences) => sequences.includes(data);
var matchesLegacyModifierSequence = (data, key, modifier) => {
  if (modifier === MODIFIERS.shift) {
    return matchesLegacySequence(data, LEGACY_SHIFT_SEQUENCES[key]);
  }
  if (modifier === MODIFIERS.ctrl) {
    return matchesLegacySequence(data, LEGACY_CTRL_SEQUENCES[key]);
  }
  return false;
};
var _lastEventType = "press";
function isKeyRelease(data) {
  if (data.includes("\x1B[200~")) {
    return false;
  }
  if (data.includes(":3u") || data.includes(":3~") || data.includes(":3A") || data.includes(":3B") || data.includes(":3C") || data.includes(":3D") || data.includes(":3H") || data.includes(":3F")) {
    return true;
  }
  return false;
}
function parseEventType(eventTypeStr) {
  if (!eventTypeStr)
    return "press";
  const eventType = parseInt(eventTypeStr, 10);
  if (eventType === 2)
    return "repeat";
  if (eventType === 3)
    return "release";
  return "press";
}
function parseKittySequence(data) {
  const csiUMatch = data.match(/^\x1b\[(\d+)(?::(\d*))?(?::(\d+))?(?:;(\d+))?(?::(\d+))?u$/);
  if (csiUMatch) {
    const codepoint = parseInt(csiUMatch[1], 10);
    const shiftedKey = csiUMatch[2] && csiUMatch[2].length > 0 ? parseInt(csiUMatch[2], 10) : undefined;
    const baseLayoutKey = csiUMatch[3] ? parseInt(csiUMatch[3], 10) : undefined;
    const modValue = csiUMatch[4] ? parseInt(csiUMatch[4], 10) : 1;
    const eventType = parseEventType(csiUMatch[5]);
    _lastEventType = eventType;
    return { codepoint, shiftedKey, baseLayoutKey, modifier: modValue - 1, eventType };
  }
  const arrowMatch = data.match(/^\x1b\[1;(\d+)(?::(\d+))?([ABCD])$/);
  if (arrowMatch) {
    const modValue = parseInt(arrowMatch[1], 10);
    const eventType = parseEventType(arrowMatch[2]);
    const arrowCodes = { A: -1, B: -2, C: -3, D: -4 };
    _lastEventType = eventType;
    return { codepoint: arrowCodes[arrowMatch[3]], modifier: modValue - 1, eventType };
  }
  const funcMatch = data.match(/^\x1b\[(\d+)(?:;(\d+))?(?::(\d+))?~$/);
  if (funcMatch) {
    const keyNum = parseInt(funcMatch[1], 10);
    const modValue = funcMatch[2] ? parseInt(funcMatch[2], 10) : 1;
    const eventType = parseEventType(funcMatch[3]);
    const funcCodes = {
      2: FUNCTIONAL_CODEPOINTS.insert,
      3: FUNCTIONAL_CODEPOINTS.delete,
      5: FUNCTIONAL_CODEPOINTS.pageUp,
      6: FUNCTIONAL_CODEPOINTS.pageDown,
      7: FUNCTIONAL_CODEPOINTS.home,
      8: FUNCTIONAL_CODEPOINTS.end
    };
    const codepoint = funcCodes[keyNum];
    if (codepoint !== undefined) {
      _lastEventType = eventType;
      return { codepoint, modifier: modValue - 1, eventType };
    }
  }
  const homeEndMatch = data.match(/^\x1b\[1;(\d+)(?::(\d+))?([HF])$/);
  if (homeEndMatch) {
    const modValue = parseInt(homeEndMatch[1], 10);
    const eventType = parseEventType(homeEndMatch[2]);
    const codepoint = homeEndMatch[3] === "H" ? FUNCTIONAL_CODEPOINTS.home : FUNCTIONAL_CODEPOINTS.end;
    _lastEventType = eventType;
    return { codepoint, modifier: modValue - 1, eventType };
  }
  return null;
}
function matchesKittySequence(data, expectedCodepoint, expectedModifier) {
  const parsed = parseKittySequence(data);
  if (!parsed)
    return false;
  const actualMod = parsed.modifier & ~LOCK_MASK;
  const expectedMod = expectedModifier & ~LOCK_MASK;
  if (actualMod !== expectedMod)
    return false;
  const normalizedCodepoint = normalizeShiftedLetterIdentityCodepoint(normalizeKittyFunctionalCodepoint(parsed.codepoint), parsed.modifier);
  const normalizedExpectedCodepoint = normalizeShiftedLetterIdentityCodepoint(normalizeKittyFunctionalCodepoint(expectedCodepoint), expectedModifier);
  if (normalizedCodepoint === normalizedExpectedCodepoint)
    return true;
  if (parsed.baseLayoutKey !== undefined && parsed.baseLayoutKey === expectedCodepoint) {
    const cp = normalizedCodepoint;
    const isLatinLetter = cp >= 97 && cp <= 122;
    const isKnownSymbol = SYMBOL_KEYS.has(String.fromCharCode(cp));
    if (!isLatinLetter && !isKnownSymbol)
      return true;
  }
  return false;
}
function parseModifyOtherKeysSequence(data) {
  const match = data.match(/^\x1b\[27;(\d+);(\d+)~$/);
  if (!match)
    return null;
  const modValue = parseInt(match[1], 10);
  const codepoint = parseInt(match[2], 10);
  return { codepoint, modifier: modValue - 1 };
}
function matchesModifyOtherKeys(data, expectedKeycode, expectedModifier) {
  const parsed = parseModifyOtherKeysSequence(data);
  if (!parsed)
    return false;
  return parsed.codepoint === expectedKeycode && parsed.modifier === expectedModifier;
}
function isWindowsTerminalSession() {
  return Boolean(process.env.WT_SESSION) && !process.env.SSH_CONNECTION && !process.env.SSH_CLIENT && !process.env.SSH_TTY;
}
function matchesRawBackspace(data, expectedModifier) {
  if (data === "\x7F")
    return expectedModifier === 0;
  if (data !== "\b")
    return false;
  return isWindowsTerminalSession() ? expectedModifier === MODIFIERS.ctrl : expectedModifier === 0;
}
function rawCtrlChar(key) {
  const char = key.toLowerCase();
  const code = char.charCodeAt(0);
  if (code >= 97 && code <= 122 || char === "[" || char === "\\" || char === "]" || char === "_") {
    return String.fromCharCode(code & 31);
  }
  if (char === "-") {
    return String.fromCharCode(31);
  }
  return null;
}
function isDigitKey(key) {
  return key >= "0" && key <= "9";
}
function matchesPrintableModifyOtherKeys(data, expectedKeycode, expectedModifier) {
  if (expectedModifier === 0)
    return false;
  const parsed = parseModifyOtherKeysSequence(data);
  if (!parsed || parsed.modifier !== expectedModifier)
    return false;
  return normalizeShiftedLetterIdentityCodepoint(parsed.codepoint, parsed.modifier) === normalizeShiftedLetterIdentityCodepoint(expectedKeycode, expectedModifier);
}
function formatKeyNameWithModifiers(keyName, modifier) {
  const mods = [];
  const effectiveMod = modifier & ~LOCK_MASK;
  const supportedModifierMask = MODIFIERS.shift | MODIFIERS.ctrl | MODIFIERS.alt | MODIFIERS.super;
  if ((effectiveMod & ~supportedModifierMask) !== 0)
    return;
  if (effectiveMod & MODIFIERS.shift)
    mods.push("shift");
  if (effectiveMod & MODIFIERS.ctrl)
    mods.push("ctrl");
  if (effectiveMod & MODIFIERS.alt)
    mods.push("alt");
  if (effectiveMod & MODIFIERS.super)
    mods.push("super");
  return mods.length > 0 ? `${mods.join("+")}+${keyName}` : keyName;
}
function parseKeyId(keyId) {
  const parts = keyId.toLowerCase().split("+");
  const key = parts[parts.length - 1];
  if (!key)
    return null;
  return {
    key,
    ctrl: parts.includes("ctrl"),
    shift: parts.includes("shift"),
    alt: parts.includes("alt"),
    super: parts.includes("super")
  };
}
function matchesKey(data, keyId) {
  const parsed = parseKeyId(keyId);
  if (!parsed)
    return false;
  const { key, ctrl, shift, alt, super: superModifier } = parsed;
  let modifier = 0;
  if (shift)
    modifier |= MODIFIERS.shift;
  if (alt)
    modifier |= MODIFIERS.alt;
  if (ctrl)
    modifier |= MODIFIERS.ctrl;
  if (superModifier)
    modifier |= MODIFIERS.super;
  switch (key) {
    case "escape":
    case "esc":
      if (modifier !== 0)
        return false;
      return data === "\x1B" || matchesKittySequence(data, CODEPOINTS.escape, 0) || matchesModifyOtherKeys(data, CODEPOINTS.escape, 0);
    case "space":
      if (!_kittyProtocolActive) {
        if (modifier === MODIFIERS.ctrl && data === "\x00") {
          return true;
        }
        if (modifier === MODIFIERS.alt && data === "\x1B ") {
          return true;
        }
      }
      if (modifier === 0) {
        return data === " " || matchesKittySequence(data, CODEPOINTS.space, 0) || matchesModifyOtherKeys(data, CODEPOINTS.space, 0);
      }
      return matchesKittySequence(data, CODEPOINTS.space, modifier) || matchesModifyOtherKeys(data, CODEPOINTS.space, modifier);
    case "tab":
      if (modifier === MODIFIERS.shift) {
        return data === "\x1B[Z" || matchesKittySequence(data, CODEPOINTS.tab, MODIFIERS.shift) || matchesModifyOtherKeys(data, CODEPOINTS.tab, MODIFIERS.shift);
      }
      if (modifier === 0) {
        return data === "\t" || matchesKittySequence(data, CODEPOINTS.tab, 0);
      }
      return matchesKittySequence(data, CODEPOINTS.tab, modifier) || matchesModifyOtherKeys(data, CODEPOINTS.tab, modifier);
    case "enter":
    case "return":
      if (modifier === MODIFIERS.shift) {
        if (matchesKittySequence(data, CODEPOINTS.enter, MODIFIERS.shift) || matchesKittySequence(data, CODEPOINTS.kpEnter, MODIFIERS.shift)) {
          return true;
        }
        if (matchesModifyOtherKeys(data, CODEPOINTS.enter, MODIFIERS.shift)) {
          return true;
        }
        if (_kittyProtocolActive) {
          return data === "\x1B\r" || data === `
`;
        }
        return false;
      }
      if (modifier === MODIFIERS.alt) {
        if (matchesKittySequence(data, CODEPOINTS.enter, MODIFIERS.alt) || matchesKittySequence(data, CODEPOINTS.kpEnter, MODIFIERS.alt)) {
          return true;
        }
        if (matchesModifyOtherKeys(data, CODEPOINTS.enter, MODIFIERS.alt)) {
          return true;
        }
        if (!_kittyProtocolActive) {
          return data === "\x1B\r";
        }
        return false;
      }
      if (modifier === 0) {
        return data === "\r" || !_kittyProtocolActive && data === `
` || data === "\x1BOM" || matchesKittySequence(data, CODEPOINTS.enter, 0) || matchesKittySequence(data, CODEPOINTS.kpEnter, 0);
      }
      return matchesKittySequence(data, CODEPOINTS.enter, modifier) || matchesKittySequence(data, CODEPOINTS.kpEnter, modifier) || matchesModifyOtherKeys(data, CODEPOINTS.enter, modifier);
    case "backspace":
      if (modifier === MODIFIERS.alt) {
        if (data === "\x1B\x7F" || data === "\x1B\b") {
          return true;
        }
        return matchesKittySequence(data, CODEPOINTS.backspace, MODIFIERS.alt) || matchesModifyOtherKeys(data, CODEPOINTS.backspace, MODIFIERS.alt);
      }
      if (modifier === MODIFIERS.ctrl) {
        if (matchesRawBackspace(data, MODIFIERS.ctrl))
          return true;
        return matchesKittySequence(data, CODEPOINTS.backspace, MODIFIERS.ctrl) || matchesModifyOtherKeys(data, CODEPOINTS.backspace, MODIFIERS.ctrl);
      }
      if (modifier === 0) {
        return matchesRawBackspace(data, 0) || matchesKittySequence(data, CODEPOINTS.backspace, 0) || matchesModifyOtherKeys(data, CODEPOINTS.backspace, 0);
      }
      return matchesKittySequence(data, CODEPOINTS.backspace, modifier) || matchesModifyOtherKeys(data, CODEPOINTS.backspace, modifier);
    case "insert":
      if (modifier === 0) {
        return matchesLegacySequence(data, LEGACY_KEY_SEQUENCES.insert) || matchesKittySequence(data, FUNCTIONAL_CODEPOINTS.insert, 0);
      }
      if (matchesLegacyModifierSequence(data, "insert", modifier)) {
        return true;
      }
      return matchesKittySequence(data, FUNCTIONAL_CODEPOINTS.insert, modifier);
    case "delete":
      if (modifier === 0) {
        return matchesLegacySequence(data, LEGACY_KEY_SEQUENCES.delete) || matchesKittySequence(data, FUNCTIONAL_CODEPOINTS.delete, 0);
      }
      if (matchesLegacyModifierSequence(data, "delete", modifier)) {
        return true;
      }
      return matchesKittySequence(data, FUNCTIONAL_CODEPOINTS.delete, modifier);
    case "clear":
      if (modifier === 0) {
        return matchesLegacySequence(data, LEGACY_KEY_SEQUENCES.clear);
      }
      return matchesLegacyModifierSequence(data, "clear", modifier);
    case "home":
      if (modifier === 0) {
        return matchesLegacySequence(data, LEGACY_KEY_SEQUENCES.home) || matchesKittySequence(data, FUNCTIONAL_CODEPOINTS.home, 0);
      }
      if (matchesLegacyModifierSequence(data, "home", modifier)) {
        return true;
      }
      return matchesKittySequence(data, FUNCTIONAL_CODEPOINTS.home, modifier);
    case "end":
      if (modifier === 0) {
        return matchesLegacySequence(data, LEGACY_KEY_SEQUENCES.end) || matchesKittySequence(data, FUNCTIONAL_CODEPOINTS.end, 0);
      }
      if (matchesLegacyModifierSequence(data, "end", modifier)) {
        return true;
      }
      return matchesKittySequence(data, FUNCTIONAL_CODEPOINTS.end, modifier);
    case "pageup":
      if (modifier === 0) {
        return matchesLegacySequence(data, LEGACY_KEY_SEQUENCES.pageUp) || matchesKittySequence(data, FUNCTIONAL_CODEPOINTS.pageUp, 0);
      }
      if (matchesLegacyModifierSequence(data, "pageUp", modifier)) {
        return true;
      }
      return matchesKittySequence(data, FUNCTIONAL_CODEPOINTS.pageUp, modifier);
    case "pagedown":
      if (modifier === 0) {
        return matchesLegacySequence(data, LEGACY_KEY_SEQUENCES.pageDown) || matchesKittySequence(data, FUNCTIONAL_CODEPOINTS.pageDown, 0);
      }
      if (matchesLegacyModifierSequence(data, "pageDown", modifier)) {
        return true;
      }
      return matchesKittySequence(data, FUNCTIONAL_CODEPOINTS.pageDown, modifier);
    case "up":
      if (modifier === MODIFIERS.alt) {
        return data === "\x1Bp" || matchesKittySequence(data, ARROW_CODEPOINTS.up, MODIFIERS.alt);
      }
      if (modifier === 0) {
        return matchesLegacySequence(data, LEGACY_KEY_SEQUENCES.up) || matchesKittySequence(data, ARROW_CODEPOINTS.up, 0);
      }
      if (matchesLegacyModifierSequence(data, "up", modifier)) {
        return true;
      }
      return matchesKittySequence(data, ARROW_CODEPOINTS.up, modifier);
    case "down":
      if (modifier === MODIFIERS.alt) {
        return data === "\x1Bn" || matchesKittySequence(data, ARROW_CODEPOINTS.down, MODIFIERS.alt);
      }
      if (modifier === 0) {
        return matchesLegacySequence(data, LEGACY_KEY_SEQUENCES.down) || matchesKittySequence(data, ARROW_CODEPOINTS.down, 0);
      }
      if (matchesLegacyModifierSequence(data, "down", modifier)) {
        return true;
      }
      return matchesKittySequence(data, ARROW_CODEPOINTS.down, modifier);
    case "left":
      if (modifier === MODIFIERS.alt) {
        return data === "\x1B[1;3D" || !_kittyProtocolActive && data === "\x1BB" || data === "\x1Bb" || matchesKittySequence(data, ARROW_CODEPOINTS.left, MODIFIERS.alt);
      }
      if (modifier === MODIFIERS.ctrl) {
        return data === "\x1B[1;5D" || matchesLegacyModifierSequence(data, "left", MODIFIERS.ctrl) || matchesKittySequence(data, ARROW_CODEPOINTS.left, MODIFIERS.ctrl);
      }
      if (modifier === 0) {
        return matchesLegacySequence(data, LEGACY_KEY_SEQUENCES.left) || matchesKittySequence(data, ARROW_CODEPOINTS.left, 0);
      }
      if (matchesLegacyModifierSequence(data, "left", modifier)) {
        return true;
      }
      return matchesKittySequence(data, ARROW_CODEPOINTS.left, modifier);
    case "right":
      if (modifier === MODIFIERS.alt) {
        return data === "\x1B[1;3C" || !_kittyProtocolActive && data === "\x1BF" || data === "\x1Bf" || matchesKittySequence(data, ARROW_CODEPOINTS.right, MODIFIERS.alt);
      }
      if (modifier === MODIFIERS.ctrl) {
        return data === "\x1B[1;5C" || matchesLegacyModifierSequence(data, "right", MODIFIERS.ctrl) || matchesKittySequence(data, ARROW_CODEPOINTS.right, MODIFIERS.ctrl);
      }
      if (modifier === 0) {
        return matchesLegacySequence(data, LEGACY_KEY_SEQUENCES.right) || matchesKittySequence(data, ARROW_CODEPOINTS.right, 0);
      }
      if (matchesLegacyModifierSequence(data, "right", modifier)) {
        return true;
      }
      return matchesKittySequence(data, ARROW_CODEPOINTS.right, modifier);
    case "f1":
    case "f2":
    case "f3":
    case "f4":
    case "f5":
    case "f6":
    case "f7":
    case "f8":
    case "f9":
    case "f10":
    case "f11":
    case "f12": {
      if (modifier !== 0) {
        return false;
      }
      const functionKey = key;
      return matchesLegacySequence(data, LEGACY_KEY_SEQUENCES[functionKey]);
    }
  }
  if (key.length === 1 && (key >= "a" && key <= "z" || isDigitKey(key) || SYMBOL_KEYS.has(key))) {
    const codepoint = key.charCodeAt(0);
    const rawCtrl = rawCtrlChar(key);
    const isLetter = key >= "a" && key <= "z";
    const isDigit = isDigitKey(key);
    if (modifier === MODIFIERS.ctrl + MODIFIERS.alt && !_kittyProtocolActive && rawCtrl) {
      if (data === `\x1B${rawCtrl}`)
        return true;
    }
    if (modifier === MODIFIERS.alt && !_kittyProtocolActive && (isLetter || isDigit || SYMBOL_KEYS.has(key))) {
      if (data === `\x1B${key}`)
        return true;
    }
    if (modifier === MODIFIERS.ctrl) {
      if (rawCtrl && data === rawCtrl)
        return true;
      return matchesKittySequence(data, codepoint, MODIFIERS.ctrl) || matchesPrintableModifyOtherKeys(data, codepoint, MODIFIERS.ctrl);
    }
    if (modifier === MODIFIERS.shift + MODIFIERS.ctrl) {
      return matchesKittySequence(data, codepoint, MODIFIERS.shift + MODIFIERS.ctrl) || matchesPrintableModifyOtherKeys(data, codepoint, MODIFIERS.shift + MODIFIERS.ctrl);
    }
    if (modifier === MODIFIERS.shift) {
      if (isLetter && data === key.toUpperCase())
        return true;
      return matchesKittySequence(data, codepoint, MODIFIERS.shift) || matchesPrintableModifyOtherKeys(data, codepoint, MODIFIERS.shift);
    }
    if (modifier !== 0) {
      return matchesKittySequence(data, codepoint, modifier) || matchesPrintableModifyOtherKeys(data, codepoint, modifier);
    }
    return data === key || matchesKittySequence(data, codepoint, 0);
  }
  return false;
}
function formatParsedKey(codepoint, modifier, baseLayoutKey) {
  const normalizedCodepoint = normalizeKittyFunctionalCodepoint(codepoint);
  const identityCodepoint = normalizeShiftedLetterIdentityCodepoint(normalizedCodepoint, modifier);
  const isLatinLetter = identityCodepoint >= 97 && identityCodepoint <= 122;
  const isDigit = identityCodepoint >= 48 && identityCodepoint <= 57;
  const isKnownSymbol = SYMBOL_KEYS.has(String.fromCharCode(identityCodepoint));
  const effectiveCodepoint = isLatinLetter || isDigit || isKnownSymbol ? identityCodepoint : baseLayoutKey ?? identityCodepoint;
  let keyName;
  if (effectiveCodepoint === CODEPOINTS.escape)
    keyName = "escape";
  else if (effectiveCodepoint === CODEPOINTS.tab)
    keyName = "tab";
  else if (effectiveCodepoint === CODEPOINTS.enter || effectiveCodepoint === CODEPOINTS.kpEnter)
    keyName = "enter";
  else if (effectiveCodepoint === CODEPOINTS.space)
    keyName = "space";
  else if (effectiveCodepoint === CODEPOINTS.backspace)
    keyName = "backspace";
  else if (effectiveCodepoint === FUNCTIONAL_CODEPOINTS.delete)
    keyName = "delete";
  else if (effectiveCodepoint === FUNCTIONAL_CODEPOINTS.insert)
    keyName = "insert";
  else if (effectiveCodepoint === FUNCTIONAL_CODEPOINTS.home)
    keyName = "home";
  else if (effectiveCodepoint === FUNCTIONAL_CODEPOINTS.end)
    keyName = "end";
  else if (effectiveCodepoint === FUNCTIONAL_CODEPOINTS.pageUp)
    keyName = "pageUp";
  else if (effectiveCodepoint === FUNCTIONAL_CODEPOINTS.pageDown)
    keyName = "pageDown";
  else if (effectiveCodepoint === ARROW_CODEPOINTS.up)
    keyName = "up";
  else if (effectiveCodepoint === ARROW_CODEPOINTS.down)
    keyName = "down";
  else if (effectiveCodepoint === ARROW_CODEPOINTS.left)
    keyName = "left";
  else if (effectiveCodepoint === ARROW_CODEPOINTS.right)
    keyName = "right";
  else if (effectiveCodepoint >= 48 && effectiveCodepoint <= 57)
    keyName = String.fromCharCode(effectiveCodepoint);
  else if (effectiveCodepoint >= 97 && effectiveCodepoint <= 122)
    keyName = String.fromCharCode(effectiveCodepoint);
  else if (SYMBOL_KEYS.has(String.fromCharCode(effectiveCodepoint)))
    keyName = String.fromCharCode(effectiveCodepoint);
  if (!keyName)
    return;
  return formatKeyNameWithModifiers(keyName, modifier);
}
function parseKey(data) {
  const kitty = parseKittySequence(data);
  if (kitty) {
    return formatParsedKey(kitty.codepoint, kitty.modifier, kitty.baseLayoutKey);
  }
  const modifyOtherKeys = parseModifyOtherKeysSequence(data);
  if (modifyOtherKeys) {
    return formatParsedKey(modifyOtherKeys.codepoint, modifyOtherKeys.modifier);
  }
  if (_kittyProtocolActive) {
    if (data === "\x1B\r" || data === `
`)
      return "shift+enter";
  }
  const legacySequenceKeyId = LEGACY_SEQUENCE_KEY_IDS[data];
  if (legacySequenceKeyId)
    return legacySequenceKeyId;
  if (data === "\x1B")
    return "escape";
  if (data === "\x1C")
    return "ctrl+\\";
  if (data === "\x1D")
    return "ctrl+]";
  if (data === "\x1F")
    return "ctrl+-";
  if (data === "\x1B\x1B")
    return "ctrl+alt+[";
  if (data === "\x1B\x1C")
    return "ctrl+alt+\\";
  if (data === "\x1B\x1D")
    return "ctrl+alt+]";
  if (data === "\x1B\x1F")
    return "ctrl+alt+-";
  if (data === "\t")
    return "tab";
  if (data === "\r" || !_kittyProtocolActive && data === `
` || data === "\x1BOM")
    return "enter";
  if (data === "\x00")
    return "ctrl+space";
  if (data === " ")
    return "space";
  if (data === "\x7F")
    return "backspace";
  if (data === "\b")
    return isWindowsTerminalSession() ? "ctrl+backspace" : "backspace";
  if (data === "\x1B[Z")
    return "shift+tab";
  if (!_kittyProtocolActive && data === "\x1B\r")
    return "alt+enter";
  if (!_kittyProtocolActive && data === "\x1B ")
    return "alt+space";
  if (data === "\x1B\x7F" || data === "\x1B\b")
    return "alt+backspace";
  if (!_kittyProtocolActive && data === "\x1BB")
    return "alt+left";
  if (!_kittyProtocolActive && data === "\x1BF")
    return "alt+right";
  if (!_kittyProtocolActive && data.length === 2 && data[0] === "\x1B") {
    const code = data.charCodeAt(1);
    if (code >= 1 && code <= 26) {
      return `ctrl+alt+${String.fromCharCode(code + 96)}`;
    }
    const key = String.fromCharCode(code);
    if (code >= 97 && code <= 122 || code >= 48 && code <= 57 || SYMBOL_KEYS.has(key)) {
      return `alt+${key}`;
    }
  }
  if (data === "\x1B[A")
    return "up";
  if (data === "\x1B[B")
    return "down";
  if (data === "\x1B[C")
    return "right";
  if (data === "\x1B[D")
    return "left";
  if (data === "\x1B[H" || data === "\x1BOH")
    return "home";
  if (data === "\x1B[F" || data === "\x1BOF")
    return "end";
  if (data === "\x1B[3~")
    return "delete";
  if (data === "\x1B[5~")
    return "pageUp";
  if (data === "\x1B[6~")
    return "pageDown";
  if (data.length === 1) {
    const code = data.charCodeAt(0);
    if (code >= 1 && code <= 26) {
      return `ctrl+${String.fromCharCode(code + 96)}`;
    }
    if (code >= 32 && code <= 126) {
      return data;
    }
  }
  return;
}
var KITTY_PRINTABLE_ALLOWED_MODIFIERS = MODIFIERS.shift | LOCK_MASK;
export {
  visibleWidth,
  truncateToWidth,
  stripTerminalSequences,
  parseKey,
  matchesKey,
  isKeyRelease,
  extractAnsiCode
};
