import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './app/App';
import './styles/tokens.css';
import './styles/app.css';
import './styles/mail.css';
import './styles/composer.css';
import './styles/messenger.css';
import './styles/notifications.css';
import './styles/contacts.css';
import './styles/settings.css';
import './styles/login.css';
import './styles/palette.css';

createRoot(document.getElementById('root')!).render(<StrictMode><App /></StrictMode>);
