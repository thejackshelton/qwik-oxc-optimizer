
import { $ } from '@qwik.dev/core';

const { a = 1, b } = { b: 2 };

console.log('root', b);

export const handler = $(() => {
  console.log('qrl', a);
});

		