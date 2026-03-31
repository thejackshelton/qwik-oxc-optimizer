
import { formAction$ } from 'forms';
import { translate } from 'i18n';

const t = translate();
export const action = formAction$((data) => {
  try {
    return { status: 'success' };
  } catch (err) {
    const t = translate();
    return { status: 'error', message: t('error-key') };
  }
});
		