import { faMap } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import { createElement } from 'react';
import { Extension, type ExtensionContext } from 'shared';
import SettingsPage from './pages/SettingsPage.tsx';
import WorldMapPage from './pages/WorldMapPage.tsx';
import { getExtTranslations } from './translations.ts';

class DevCaloptreyxWorldMapExtension extends Extension {
  public cardIcon = createElement(FontAwesomeIcon, { icon: faMap });
  public cardConfigurationPage: React.FC | null = SettingsPage;

  public initialize(ctx: ExtensionContext): void {
    ctx.extensionRegistry.enterRoutes((routes) =>
      routes.addServerRoute({
        name: () => getExtTranslations().t('title', {}),
        icon: faMap,
        path: '/world-map',
        element: WorldMapPage,
        permission: 'world-map.view',
      }),
    );

    ctx.extensionRegistry.enterPermissionIcons((icons) =>
      icons.addServerPermissionIcon('world-map', createElement(FontAwesomeIcon, { icon: faMap })),
    );
  }
}

export default new DevCaloptreyxWorldMapExtension();
