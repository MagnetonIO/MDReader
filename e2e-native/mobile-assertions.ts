/**
 * WebKit may concatenate adjacent button text while Chromium separates it with
 * line breaks. The semantic order is the contract, not driver whitespace.
 */
export const MOBILE_NAVIGATION_TEXT_PATTERN =
  /^Library\s*Search\s*Sources\s*Settings$/;
