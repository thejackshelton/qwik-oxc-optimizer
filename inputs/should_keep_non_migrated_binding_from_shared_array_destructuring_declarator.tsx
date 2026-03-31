
import { $ } from '@qwik.dev/core';

const [a, b] = [1, 2];

console.log('root', b);

export const handler = $(() => {
  console.log('qrl', a);
});

		