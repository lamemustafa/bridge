// SPDX-License-Identifier: Apache-2.0

export function canStartOutstandingsRead(
  company: { name: string; guid: string } | undefined,
  inrAsserted: boolean,
) {
  return company !== undefined && inrAsserted;
}
