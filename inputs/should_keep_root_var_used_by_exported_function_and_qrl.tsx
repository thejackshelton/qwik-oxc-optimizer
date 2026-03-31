
import { $ } from '@qwik.dev/core';

const shared = {
  id: 'abc',
};

export function readShared() {
  return shared.id;
}

export const handler = $(() => {
  console.log(shared.id);
});

		