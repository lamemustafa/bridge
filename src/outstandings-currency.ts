// SPDX-License-Identifier: Apache-2.0

export function canStartOutstandingsRead(
  company: { name: string; guid: string } | undefined,
  inrAssertedCompanyGuid: string | null,
) {
  return company?.guid === inrAssertedCompanyGuid;
}
