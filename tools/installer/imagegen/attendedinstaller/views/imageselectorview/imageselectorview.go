// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

package imageselectorview

import (
	"fmt"

	"installer/imagegen/attendedinstaller/primitives/customshortcutlist"
	"installer/imagegen/attendedinstaller/primitives/navigationbar"
	"installer/imagegen/attendedinstaller/uitext"
	"installer/imagegen/attendedinstaller/uiutils"
	"installer/imagegen/configuration"
	"installer/imagegen/imageutils"
	"installer/internal/logger"

	"github.com/gdamore/tcell"
	"github.com/rivo/tview"
)

// UI constants.
const (
	// default to <Next>
	defaultNavButton = 1
	defaultPadding   = 1

	listProportion = 0

	navBarHeight     = 0
	navBarProportion = 1
)

type ImageSelectorView struct {
	optionList      *customshortcutlist.List
	navBar          *navigationbar.NavigationBar
	flex            *tview.Flex
	centeredFlex    *tview.Flex
	availableImages []imageutils.SystemImage
	needsToPrompt   bool
	tridentConfig   *configuration.TridentConfigData
}

func New(availableImages []imageutils.SystemImage, tridentConfig *configuration.TridentConfigData) *ImageSelectorView {
	iv := &ImageSelectorView{
		availableImages: availableImages,
		tridentConfig:   tridentConfig,
	}

	iv.needsToPrompt = (len(iv.availableImages) > 1)

	if !iv.needsToPrompt {
		// Auto-select the only available image
		iv.applySelection(0)
	}

	return iv
}

func (iv *ImageSelectorView) Initialize(tridentConfig *configuration.TridentConfigData, backButtonText string, app *tview.Application, nextPage, previousPage, quit, refreshTitle func()) (err error) {
	iv.navBar = navigationbar.NewNavigationBar().
		AddButton(uitext.ButtonGoBack, previousPage).
		AddButton(uitext.ButtonNext, func() {
			iv.applySelection(iv.optionList.GetCurrentItem())
			nextPage()
		}).
		SetAlign(tview.AlignCenter)

	iv.optionList = customshortcutlist.NewList().
		ShowSecondaryText(false)

	err = iv.populateImageOptions()
	if err != nil {
		return
	}

	listWidth, listHeight := uiutils.MinListSize(iv.optionList)
	centeredList := uiutils.Center(listWidth, listHeight, iv.optionList)

	iv.flex = tview.NewFlex().
		SetDirection(tview.FlexRow).
		AddItem(centeredList, listHeight, listProportion, true).
		AddItem(iv.navBar, navBarHeight, navBarProportion, false)

	iv.centeredFlex = uiutils.CenterVerticallyDynamically(iv.flex)

	// Box styling
	iv.optionList.SetBorderPadding(defaultPadding, defaultPadding, defaultPadding, defaultPadding)

	iv.centeredFlex.SetBackgroundColor(tview.Styles.PrimitiveBackgroundColor)

	return
}

func (iv *ImageSelectorView) HandleInput(event *tcell.EventKey) *tcell.EventKey {
	if iv.navBar.UnfocusedInputHandler(event) {
		return nil
	}

	return event
}

// NeedsToPrompt returns true if this view should be shown to the user so an image can be selected.
func (iv *ImageSelectorView) NeedsToPrompt() bool {
	return iv.needsToPrompt
}

// Resets the page by resetting all user selection
func (iv *ImageSelectorView) Reset() (err error) {
	iv.navBar.ClearUserFeedback()
	iv.navBar.SetSelectedButton(defaultNavButton)

	iv.optionList.SetCurrentItem(0)

	return
}

func (iv *ImageSelectorView) Name() string {
	return "IMAGE_SELECTION"
}

func (iv *ImageSelectorView) Title() string {
	return uitext.ImageSelectionTitle
}

func (iv *ImageSelectorView) Primitive() tview.Primitive {
	return iv.centeredFlex
}

func (iv *ImageSelectorView) OnShow() {
}

func (iv *ImageSelectorView) applySelection(selectedIndex int) {
	// Clear any previous feedback (if navBar has been initialized)
	if iv.navBar != nil {
		iv.navBar.ClearUserFeedback()
	}

	selectedImage := iv.availableImages[selectedIndex]

	// Store the corresponding url of the selected image in the TridentConfigData
	iv.tridentConfig.ImagePath = selectedImage.URL
	logger.Log.Infof("Image selected: %s (url: %s)", selectedImage.Name, selectedImage.URL)
}

func (iv *ImageSelectorView) populateImageOptions() (err error) {
	if len(iv.availableImages) == 0 {
		errMsg := "No images available for selection"
		logger.Log.Errorf(errMsg)
		iv.navBar.SetUserFeedback(errMsg, tview.Styles.TertiaryTextColor)
		return fmt.Errorf("no images available for selection")
	}

	for _, image := range iv.availableImages {
		iv.optionList.AddItem(image.Name, "", 0, nil)
	}

	return
}
