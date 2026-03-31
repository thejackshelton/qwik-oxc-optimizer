
import { $ } from '@qwik.dev/core';

const shared = {
  id: 'abc',
};

export const exportedValue = shared.id;

export const handler = $(() => {
  console.log(shared.id);
});

		