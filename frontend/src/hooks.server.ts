// 服务端钩子（可选，用于 SSR）
import type { Handle } from '@sveltejs/kit';

export const handle: Handle = async ({ event, resolve }) => {
	// 这里可以添加服务端的认证逻辑
	return resolve(event);
};
