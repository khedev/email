import { useEffect, useState } from 'react';
import { Bell, CheckCheck } from 'lucide-react';
import { getNotifications, markAllNotificationsRead, type AppNotification } from '../platform/tauri';

export function NotificationPanel({ onClose }: { onClose: () => void }) {
  const [items, setItems] = useState<AppNotification[]>([]); const [error, setError] = useState('');
  useEffect(() => { getNotifications().then(setItems).catch(() => setError('Unable to load notifications.')); }, []);
  const markRead = () => markAllNotificationsRead().then(() => { setItems([]); }).catch(() => setError('Unable to update notifications.'));
  return <section className="notification-panel" aria-label="Notifications"><header><div><Bell size={17}/><strong>Notifications</strong></div><button onClick={markRead}><CheckCheck size={15}/> Mark all read</button></header>{error && <p className="notification-error">{error}</p>}{items.length ? <div>{items.map(item => <article key={item.id}><strong>{item.title}</strong><p>{item.body}</p><time>{formatTime(item.createdAt)}</time></article>)}</div> : <p className="notification-empty">You are all caught up.</p>}<button className="close-notifications" onClick={onClose}>Close</button></section>;
}
function formatTime(value: string) { const date = new Date(value); return Number.isNaN(date.valueOf()) ? value : new Intl.DateTimeFormat(undefined, { hour: 'numeric', minute: '2-digit' }).format(date); }
